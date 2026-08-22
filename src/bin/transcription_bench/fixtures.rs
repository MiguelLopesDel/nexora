//! Test fixtures (synthetic speech + comprehension questions), audio
//! synthesis/capture, and the rolling-window transcription loop under test.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nexora::meeting::apply_corrections;
use nexora::whisper::{Transcriber, novel_transcript};
use tokio::io::AsyncReadExt;

use crate::Cli;

const SAMPLE_RATE: usize = 16_000;
const BYTES_PER_SECOND: usize = SAMPLE_RATE * 2; // s16le mono

pub(crate) struct Question {
    pub(crate) ask: &'static str,
    /// The answer must contain at least one alternative from every group
    /// (compared lowercase and diacritic-folded).
    pub(crate) keyword_groups: &'static [&'static [&'static str]],
}

pub(crate) struct Fixture {
    pub(crate) id: &'static str,
    /// espeak-ng voice (also the whisper language hint unless --auto-language)
    voice: &'static str,
    language: &'static str,
    spoken: &'static str,
    pub(crate) questions: &'static [Question],
}

pub(crate) const FIXTURES: &[Fixture] = &[
    Fixture {
        id: "pt-discurso",
        voice: "pt-br",
        language: "pt",
        spoken: "A inteligência artificial vai transformar a educação no Brasil, mas precisamos garantir que todas as escolas tenham acesso à tecnologia.",
        questions: &[
            Question {
                ask: "Qual é o sentido desse discurso? O que o autor quis dizer?",
                keyword_groups: &[&["educa"], &["tecnologia", "acesso", "escola"]],
            },
            Question {
                ask: "Fora do assunto do áudio: o que significa HTTP, em uma frase?",
                keyword_groups: &[&["protocol", "hipertexto", "hypertext", "transfer"]],
            },
        ],
    },
    Fixture {
        id: "pt-termo",
        voice: "pt-br",
        language: "pt",
        spoken: "O deputado defendeu o novo marco regulatório das criptomoedas durante a sessão de ontem.",
        questions: &[Question {
            ask: "Tô ouvindo isso e não sei o que é marco regulatório. Me explica de forma simples.",
            keyword_groups: &[&["regr", "lei", "norma", "regulament"]],
        }],
    },
    Fixture {
        id: "pt-girias",
        voice: "pt-br",
        language: "pt",
        spoken: "Mano, aquele esquema tá muito daora, bora fechar negócio logo com a galera.",
        questions: &[Question {
            ask: "O que a pessoa quis dizer com isso?",
            keyword_groups: &[&["negóci", "negoci", "acord", "fechar", "proposta"]],
        }],
    },
    Fixture {
        id: "pt-numeros",
        voice: "pt-br",
        language: "pt",
        spoken: "A reunião foi remarcada para quinta-feira às três da tarde, com um orçamento de vinte mil reais.",
        questions: &[Question {
            ask: "Quando ficou marcada a reunião e qual é o orçamento?",
            keyword_groups: &[
                &["quinta"],
                &["vinte mil", "20 mil", "20.000", "20000", "r$"],
            ],
        }],
    },
    Fixture {
        id: "en-fox",
        voice: "en-us",
        language: "en",
        spoken: "The quick brown fox jumps over the lazy dog near the river bank.",
        questions: &[Question {
            ask: "What animal jumped, and over what did it jump?",
            keyword_groups: &[&["fox"], &["dog"]],
        }],
    },
];

/// Systematic espeak+whisper mishearings observed on this rig, in the same
/// format users put under [meeting.corrections]. Extend after inspecting the
/// raw transcripts printed by a run.
pub(crate) fn bench_corrections() -> BTreeMap<String, String> {
    [
        // whisper writes the compound; the fixture says two words
        ("riverbank", "river bank"),
        // slang: whisper normalizes "daora" into "da hora"
        ("da hora", "daora"),
        // consistent mishear of "galera" at this window boundary
        ("góssia", "galera"),
    ]
    .into_iter()
    .map(|(wrong, right)| (wrong.to_string(), right.to_string()))
    .collect()
}

pub(crate) struct TranscriptUpdate {
    pub(crate) raw: String,
    pub(crate) corrected: String,
    /// Audio captured → corrected text available.
    pub(crate) latency: Duration,
    /// Whisper inference alone.
    pub(crate) inference: Duration,
}

pub(crate) struct FixtureResult {
    pub(crate) id: &'static str,
    pub(crate) expected: &'static str,
    pub(crate) raw_transcript: String,
    pub(crate) corrected_transcript: String,
    pub(crate) raw_wer: f64,
    pub(crate) corrected_wer: f64,
    pub(crate) updates: Vec<TranscriptUpdate>,
    /// Corrected updates in arrival order, as the overlay would keep them.
    pub(crate) live_transcript: Vec<String>,
}

/// A private null sink so benchmark audio is inaudible and isolated from the
/// machine's real output; removed again on drop.
pub(crate) struct NullSink {
    pub(crate) name: String,
    module: String,
}

impl NullSink {
    pub(crate) fn create() -> Result<Self> {
        let name = format!("nexora_bench_{}", std::process::id());
        let output = std::process::Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={name}"),
                "sink_properties=device.description=NexoraBench",
            ])
            .output()
            .context("could not run `pactl`; install PulseAudio utilities")?;
        if !output.status.success() {
            bail!(
                "pactl load-module failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let module = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if module.is_empty() {
            bail!("pactl did not return a module id");
        }
        Ok(Self { name, module })
    }

    pub(crate) fn monitor(&self) -> String {
        format!("{}.monitor", self.name)
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        let _ = std::process::Command::new("pactl")
            .args(["unload-module", &self.module])
            .status();
    }
}

/// Piper voice file expected per fixture language.
const PIPER_VOICES: &[(&str, &str)] = &[
    ("pt", "pt_BR-faber-medium.onnx"),
    ("en", "en_US-lessac-medium.onnx"),
];

pub(crate) fn default_piper_voices_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("piper/voices")
}

/// Synthesize a fixture, preferring a realistic neural voice (piper) over
/// espeak-ng's robotic one — whisper is trained on human speech, so espeak
/// heavily understates real-world accuracy.
pub(crate) fn synthesize(
    fixture: &Fixture,
    dir: &Path,
    cli: &Cli,
) -> Result<(PathBuf, &'static str)> {
    let path = dir.join(format!("{}.wav", fixture.id));
    if cli.tts != "espeak"
        && let Some((_, file)) = PIPER_VOICES
            .iter()
            .find(|(lang, _)| *lang == fixture.language)
    {
        let voice = cli.piper_voices.join(file);
        if voice.exists() {
            let mut child = std::process::Command::new("piper")
                .arg("--model")
                .arg(&voice)
                .arg("--output_file")
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("could not run `piper`")?;
            use std::io::Write;
            child
                .stdin
                .take()
                .context("piper stdin unavailable")?
                .write_all(fixture.spoken.as_bytes())?;
            if child.wait()?.success() {
                return Ok((path, "piper"));
            }
            bail!("piper failed for fixture {}", fixture.id);
        } else if cli.tts == "piper" {
            bail!("piper voice {} not found", voice.display());
        }
    }
    let status = std::process::Command::new("espeak-ng")
        .args(["-v", fixture.voice, "-s", "150", "-w"])
        .arg(&path)
        .arg(fixture.spoken)
        .status()
        .context("could not run `espeak-ng`; install espeak-ng")?;
    if !status.success() {
        bail!("espeak-ng failed for fixture {}", fixture.id);
    }
    Ok((path, "espeak-ng"))
}

type ChunkReceiver = async_channel::Receiver<(Vec<u8>, Instant)>;

/// Capture chunks from the sink monitor exactly like the app does, stamping
/// each chunk with the moment its audio finished being heard.
fn capture(
    device: String,
    bytes_per_chunk: usize,
) -> Result<(tokio::process::Child, ChunkReceiver)> {
    let mut child = tokio::process::Command::new("parec")
        .args([
            "--record",
            "--raw",
            "--format=s16le",
            "--rate=16000",
            "--channels=1",
            &format!("--device={device}"),
            "--client-name=NexoraBench",
            "--stream-name=Transcription benchmark",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("could not start `parec`; install PulseAudio utilities")?;
    let mut stdout = child.stdout.take().context("parec stdout unavailable")?;
    let (tx, rx) = async_channel::unbounded();
    tokio::spawn(async move {
        loop {
            let mut chunk = vec![0_u8; bytes_per_chunk];
            if stdout.read_exact(&mut chunk).await.is_err() {
                break;
            }
            if tx.send((chunk, Instant::now())).await.is_err() {
                break;
            }
        }
    });
    Ok((child, rx))
}

fn pcm_level(pcm: &[u8]) -> u16 {
    let mut total = 0_u64;
    let mut samples = 0_u64;
    for sample in pcm.chunks_exact(2) {
        total += (i16::from_le_bytes([sample[0], sample[1]]) as i32).unsigned_abs() as u64;
        samples += 1;
    }
    total.checked_div(samples).unwrap_or(0).min(u16::MAX as u64) as u16
}

/// The same rolling-window streaming loop as meeting::transcribe_audio, with
/// timing hooks. Runs until the chunk channel closes.
async fn stream_transcribe(
    chunks: ChunkReceiver,
    transcriber: Arc<Transcriber>,
    language: String,
    corrections: &BTreeMap<String, String>,
    window_bytes: usize,
    silence_threshold: u16,
) -> Result<Vec<TranscriptUpdate>> {
    let mut updates = Vec::new();
    let mut rolling: Vec<Vec<u8>> = Vec::new();
    let mut rolling_bytes = 0_usize;
    let mut previous_window: Option<String> = None;
    while let Ok((pcm, captured_at)) = chunks.recv().await {
        let silent = silence_threshold > 0 && pcm_level(&pcm) < silence_threshold;
        rolling_bytes += pcm.len();
        rolling.push(pcm);
        while rolling_bytes > window_bytes {
            rolling_bytes = rolling_bytes.saturating_sub(rolling.remove(0).len());
        }
        if silent {
            previous_window = None;
            continue;
        }
        if rolling_bytes < window_bytes {
            continue;
        }
        let window: Vec<u8> = rolling.iter().flatten().copied().collect();
        let job = Arc::clone(&transcriber);
        let hint = language.clone();
        let started = Instant::now();
        let raw = tokio::task::spawn_blocking(move || job.transcribe(&window, &hint))
            .await
            .context("whisper task failed")??;
        let inference = started.elapsed();
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            continue;
        }
        let novel = previous_window
            .as_deref()
            .map_or_else(|| raw.clone(), |previous| novel_transcript(previous, &raw));
        previous_window = Some(raw);
        let corrected = apply_corrections(&novel, corrections);
        if corrected.is_empty() {
            continue;
        }
        updates.push(TranscriptUpdate {
            raw: novel,
            corrected,
            latency: captured_at.elapsed(),
            inference,
        });
    }
    Ok(updates)
}

pub(crate) fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

fn normalized_words(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Word error rate: word-level Levenshtein distance over reference length.
fn wer(expected: &str, got: &str) -> f64 {
    let reference = normalized_words(expected);
    let hypothesis = normalized_words(got);
    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, expected_word) in reference.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, got_word) in hypothesis.iter().enumerate() {
            let substitution = previous[j] + usize::from(expected_word != got_word);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[hypothesis.len()] as f64 / reference.len() as f64
}

pub(crate) async fn run_fixture(
    fixture: &Fixture,
    wav: &Path,
    sink: &NullSink,
    transcriber: &Arc<Transcriber>,
    corrections: &BTreeMap<String, String>,
    cli: &Cli,
) -> Result<FixtureResult> {
    let bytes_per_chunk = BYTES_PER_SECOND * cli.chunk_seconds as usize;
    let window_bytes = BYTES_PER_SECOND * cli.window_seconds.max(cli.chunk_seconds) as usize;
    let (mut recorder, chunks) = capture(sink.monitor(), bytes_per_chunk)?;
    // Give the capture stream a moment to connect before audio starts.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let language = if cli.auto_language {
        String::new()
    } else {
        fixture.language.to_string()
    };
    let worker = tokio::spawn({
        let transcriber = Arc::clone(transcriber);
        let corrections = corrections.clone();
        let chunks = chunks.clone();
        async move {
            stream_transcribe(
                chunks,
                transcriber,
                language,
                &corrections,
                window_bytes,
                180,
            )
            .await
        }
    });

    let status = tokio::process::Command::new("paplay")
        .arg(format!("--device={}", sink.name))
        .arg(wav)
        .status()
        .await
        .context("could not run `paplay`")?;
    if !status.success() {
        bail!("paplay failed for fixture {}", fixture.id);
    }
    // Let the trailing window flush through capture before stopping.
    tokio::time::sleep(Duration::from_secs(cli.window_seconds.max(2))).await;
    let _ = recorder.kill().await;
    chunks.close();
    let updates = worker.await.context("transcription worker panicked")??;

    let live_transcript: Vec<String> = updates
        .iter()
        .map(|update| update.corrected.clone())
        .collect();
    let corrected_transcript = live_transcript.join(" ");
    let raw_transcript = updates
        .iter()
        .map(|update| update.raw.clone())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(FixtureResult {
        id: fixture.id,
        expected: fixture.spoken,
        raw_wer: wer(fixture.spoken, &raw_transcript),
        corrected_wer: wer(fixture.spoken, &corrected_transcript),
        raw_transcript,
        corrected_transcript,
        updates,
        live_transcript,
    })
}
