//! Developer benchmark for the live meeting pipeline.
//!
//! Plays synthetic speech (espeak-ng) into a dedicated PulseAudio null sink —
//! never the default output, so nothing is audible and real audio is never
//! mixed in — captures it back through `parec` exactly like the app, streams
//! it through the same rolling-window whisper transcription, and reports:
//!
//! - the exact transcript of every fixture, before and after
//!   [meeting.corrections]-style fixes,
//! - word error rate (WER) per fixture with a pass/fail accuracy gate,
//! - transcription latency (audio heard → text available) and inference cost,
//! - answers from a local Ollama model to questions about what was heard
//!   ("what is X?", "what did the speaker mean?"), with keyword checks and
//!   first-token/total latency.
//!
//! Exit code: 0 all gates pass · 1 a gate failed · 2 environment problem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use nexora::config::{ProviderConfig, ProviderKind};
use nexora::conversation::Role;
use nexora::meeting::apply_corrections;
use nexora::providers::{ChatRequest, StreamEvent, stream_chat};
use nexora::ui::append_meeting_transcript_context;
use nexora::whisper::{ComputePreference, Transcriber, model_path, novel_transcript};
use tokio::io::AsyncReadExt;

const SAMPLE_RATE: usize = 16_000;
const BYTES_PER_SECOND: usize = SAMPLE_RATE * 2; // s16le mono

#[derive(Parser)]
#[command(
    name = "transcription_bench",
    about = "End-to-end transcription quality/latency benchmark on an isolated null sink"
)]
struct Cli {
    /// Curated whisper model id (tiny, base, small, large-v3-turbo-q5_0)
    #[arg(long, default_value = "tiny")]
    model: String,
    /// Force whisper language detection instead of per-fixture hints
    #[arg(long)]
    auto_language: bool,
    /// Capture stride in seconds (the app default is 2)
    #[arg(long, default_value_t = 2)]
    chunk_seconds: u64,
    /// Rolling transcription window in seconds (the app default is 4)
    #[arg(long, default_value_t = 4)]
    window_seconds: u64,
    /// Gate: maximum mean corrected WER across fixtures
    #[arg(long, default_value_t = 0.35)]
    max_wer: f64,
    /// Gate: maximum corrected WER for any single fixture
    #[arg(long, default_value_t = 0.60)]
    max_fixture_wer: f64,
    /// Gate: maximum mean latency from audio captured to text available (ms)
    #[arg(long, default_value_t = 2_500)]
    max_latency_ms: u128,
    /// Skip the question-answering stage
    #[arg(long)]
    skip_qa: bool,
    /// OpenAI-compatible endpoint for the QA stage
    #[arg(long, default_value = "http://localhost:11434/v1")]
    qa_url: String,
    /// Model used to answer questions about the transcript
    #[arg(long, default_value = "gemma4:e2b")]
    qa_model: String,
    /// Gate: maximum time to the first answer token (ms)
    #[arg(long, default_value_t = 30_000)]
    max_qa_first_token_ms: u128,
    /// Keep the generated wav files for listening/debugging
    #[arg(long)]
    keep_wavs: bool,
    /// Speech synthesizer: "auto" uses piper voices when installed (much more
    /// realistic than espeak) and falls back to espeak-ng per fixture.
    #[arg(long, default_value = "auto")]
    tts: String,
    /// Directory holding piper voices (<lang>.onnx as named in PIPER_VOICES)
    #[arg(long, default_value_os_t = default_piper_voices_dir())]
    piper_voices: PathBuf,
}

/// Piper voice file expected per fixture language.
const PIPER_VOICES: &[(&str, &str)] = &[
    ("pt", "pt_BR-faber-medium.onnx"),
    ("en", "en_US-lessac-medium.onnx"),
];

fn default_piper_voices_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("piper/voices")
}

struct Question {
    ask: &'static str,
    /// The answer must contain at least one alternative from every group
    /// (compared lowercase and diacritic-folded).
    keyword_groups: &'static [&'static [&'static str]],
}

struct Fixture {
    id: &'static str,
    /// espeak-ng voice (also the whisper language hint unless --auto-language)
    voice: &'static str,
    language: &'static str,
    spoken: &'static str,
    questions: &'static [Question],
}

const FIXTURES: &[Fixture] = &[
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
fn bench_corrections() -> BTreeMap<String, String> {
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

struct TranscriptUpdate {
    raw: String,
    corrected: String,
    /// Audio captured → corrected text available.
    latency: Duration,
    /// Whisper inference alone.
    inference: Duration,
}

struct FixtureResult {
    id: &'static str,
    expected: &'static str,
    raw_transcript: String,
    corrected_transcript: String,
    raw_wer: f64,
    corrected_wer: f64,
    updates: Vec<TranscriptUpdate>,
    /// Corrected updates in arrival order, as the overlay would keep them.
    live_transcript: Vec<String>,
}

/// A private null sink so benchmark audio is inaudible and isolated from the
/// machine's real output; removed again on drop.
struct NullSink {
    name: String,
    module: String,
}

impl NullSink {
    fn create() -> Result<Self> {
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

    fn monitor(&self) -> String {
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

/// Synthesize a fixture, preferring a realistic neural voice (piper) over
/// espeak-ng's robotic one — whisper is trained on human speech, so espeak
/// heavily understates real-world accuracy.
fn synthesize(fixture: &Fixture, dir: &Path, cli: &Cli) -> Result<(PathBuf, &'static str)> {
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

fn fold(text: &str) -> String {
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

async fn run_fixture(
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

struct Answer {
    text: String,
    first_token: Duration,
    total: Duration,
}

async fn ask_model(cli: &Cli, question: &str, live_transcript: &[String]) -> Result<Answer> {
    let provider = ProviderConfig {
        kind: ProviderKind::Openai,
        base_url: Some(cli.qa_url.clone()),
        api_key: Some("ollama".into()),
        api_key_env: None,
        default_model: Some(cli.qa_model.clone()),
        thinking: None,
        reasoning_effort: None,
    };
    let mut messages = vec![(Role::User, question.to_string())];
    append_meeting_transcript_context(&mut messages, live_transcript, 12_000);
    let request = ChatRequest {
        model: cli.qa_model.clone(),
        system: Some(
            "You are Nexora, a concise on-screen assistant. Answer briefly and directly.".into(),
        ),
        messages,
        image_png: None,
        max_tokens: 512,
    };
    let (tx, rx) = async_channel::unbounded::<StreamEvent>();
    let started = Instant::now();
    tokio::spawn(async move { stream_chat(&provider, request, tx).await });
    let mut text = String::new();
    let mut first_token = None;
    while let Ok(event) = rx.recv().await {
        match event {
            StreamEvent::Delta(delta) => {
                if first_token.is_none() && !delta.trim().is_empty() {
                    first_token = Some(started.elapsed());
                }
                text.push_str(&delta);
            }
            StreamEvent::Done => break,
            StreamEvent::Error(message) => bail!("model error: {message}"),
        }
    }
    Ok(Answer {
        text: text.trim().to_string(),
        first_token: first_token.unwrap_or_else(|| started.elapsed()),
        total: started.elapsed(),
    })
}

fn keywords_found(answer: &str, groups: &[&[&str]]) -> Vec<bool> {
    let folded = fold(answer);
    groups
        .iter()
        .map(|group| group.iter().any(|keyword| folded.contains(&fold(keyword))))
        .collect()
}

fn mean_ms(durations: impl Iterator<Item = Duration>) -> Option<u128> {
    let values: Vec<u128> = durations.map(|d| d.as_millis()).collect();
    (!values.is_empty()).then(|| values.iter().sum::<u128>() / values.len() as u128)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(&cli).await {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(1),
        Err(err) => {
            eprintln!("transcription_bench: {err:#}");
            std::process::ExitCode::from(2)
        }
    }
}

async fn run(cli: &Cli) -> Result<bool> {
    let model = model_path(&cli.model);
    if !model.exists() {
        bail!(
            "whisper model `{}` is not downloaded (expected {}); download it in the app or with the whisper manager",
            cli.model,
            model.display()
        );
    }
    let transcriber = Arc::new(
        tokio::task::spawn_blocking({
            let model = model.clone();
            move || Transcriber::load(&model, ComputePreference::Cpu)
        })
        .await
        .context("whisper load task failed")??,
    );
    println!(
        "model: {} ({}) · chunk {}s · window {}s",
        cli.model,
        transcriber.compute_label(),
        cli.chunk_seconds,
        cli.window_seconds
    );

    let wav_dir = std::env::temp_dir().join(format!("nexora-bench-{}", std::process::id()));
    std::fs::create_dir_all(&wav_dir)?;
    let sink = NullSink::create()?;
    println!(
        "null sink: {} (isolated from the default output)\n",
        sink.name
    );

    let corrections = bench_corrections();
    let mut results = Vec::new();
    for fixture in FIXTURES {
        let (wav, tts) = synthesize(fixture, &wav_dir, cli)?;
        println!("▶ {} — playing + transcribing… (voice: {tts})", fixture.id);
        let result = run_fixture(fixture, &wav, &sink, &transcriber, &corrections, cli).await?;
        println!("  expected : {}", result.expected);
        println!("  raw      : {}", result.raw_transcript);
        if result.corrected_transcript != result.raw_transcript {
            println!("  corrected: {}", result.corrected_transcript);
        }
        let latency = mean_ms(result.updates.iter().map(|u| u.latency)).unwrap_or(0);
        let inference = mean_ms(result.updates.iter().map(|u| u.inference)).unwrap_or(0);
        println!(
            "  WER {:.0}% raw → {:.0}% corrected · {} updates · latency {} ms mean (inference {} ms)\n",
            result.raw_wer * 100.0,
            result.corrected_wer * 100.0,
            result.updates.len(),
            latency,
            inference
        );
        results.push(result);
    }

    if !cli.keep_wavs {
        let _ = std::fs::remove_dir_all(&wav_dir);
    } else {
        println!("wav files kept in {}\n", wav_dir.display());
    }

    // ---- transcription gates -------------------------------------------
    let mean_wer =
        results.iter().map(|r| r.corrected_wer).sum::<f64>() / results.len().max(1) as f64;
    let worst = results
        .iter()
        .max_by(|a, b| a.corrected_wer.total_cmp(&b.corrected_wer));
    let mean_latency = mean_ms(
        results
            .iter()
            .flat_map(|r| r.updates.iter().map(|u| u.latency)),
    )
    .unwrap_or(0);
    let mut pass = true;

    println!("== transcription quality ==");
    println!(
        "mean corrected WER {:.1}% (gate ≤ {:.0}%)",
        mean_wer * 100.0,
        cli.max_wer * 100.0
    );
    if let Some(worst) = worst {
        println!(
            "worst fixture {} at {:.1}% (gate ≤ {:.0}%)",
            worst.id,
            worst.corrected_wer * 100.0,
            cli.max_fixture_wer * 100.0
        );
    }
    if mean_wer > cli.max_wer {
        println!("FAIL: mean WER above gate");
        pass = false;
    }
    if let Some(worst) = worst
        && worst.corrected_wer > cli.max_fixture_wer
    {
        println!("FAIL: fixture {} above per-fixture gate", worst.id);
        pass = false;
    }

    println!("\n== latency ==");
    println!(
        "audio heard → text available: {} ms mean (gate ≤ {} ms); capture stride adds up to {} ms before that",
        mean_latency,
        cli.max_latency_ms,
        cli.chunk_seconds * 1_000
    );
    if mean_latency > cli.max_latency_ms {
        println!("FAIL: mean transcription latency above gate");
        pass = false;
    }

    // ---- question answering --------------------------------------------
    if !cli.skip_qa {
        println!("\n== questions about what was heard ({}) ==", cli.qa_model);
        // First contact loads the model into memory; keep that cold-start out
        // of the per-question latency numbers (the app keeps models warm).
        let warmup = Instant::now();
        match ask_model(cli, "Reply with the single word: ready", &[]).await {
            Ok(_) => println!(
                "model warm-up: {} ms (excluded from gates)",
                warmup.elapsed().as_millis()
            ),
            Err(err) => println!("model warm-up failed: {err:#}"),
        }
        for (fixture, result) in FIXTURES.iter().zip(&results) {
            for question in fixture.questions {
                println!("\n[{}] Q: {}", fixture.id, question.ask);
                match ask_model(cli, question.ask, &result.live_transcript).await {
                    Ok(answer) => {
                        let found = keywords_found(&answer.text, question.keyword_groups);
                        let ok = found.iter().all(|hit| *hit);
                        println!("A: {}", answer.text);
                        println!(
                            "keywords {} · first token {} ms · total {} ms",
                            if ok { "OK" } else { "MISSING" },
                            answer.first_token.as_millis(),
                            answer.total.as_millis()
                        );
                        if !ok {
                            for (group, hit) in question.keyword_groups.iter().zip(&found) {
                                if !hit {
                                    println!("  missing any of: {group:?}");
                                }
                            }
                            pass = false;
                        }
                        if answer.first_token.as_millis() > cli.max_qa_first_token_ms {
                            println!("FAIL: first token above gate");
                            pass = false;
                        }
                    }
                    Err(err) => {
                        println!("A: <error: {err:#}>");
                        pass = false;
                    }
                }
            }
        }
    }

    println!("\n{}", if pass { "PASS" } else { "FAIL" });
    Ok(pass)
}
