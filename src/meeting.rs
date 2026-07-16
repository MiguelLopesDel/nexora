//! Opt-in live meeting pipeline: Pulse/PipeWire capture, transcription,
//! translation, coaching, screen context, notes, and final summary.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use reqwest::multipart::{Form, Part};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

use crate::config::{
    AssistantProfile, MeetingConfig, ProviderConfig, ProviderKind, TaskConfig, VisionConfig,
};
use crate::conversation::Role;
use crate::providers::{ChatRequest, complete_chat};
use crate::screenshot;
use crate::vision;
use crate::whisper;

const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Status(String),
    Transcript(String),
    Translation(String),
    Insight(String),
    Summary(String),
    Error(String),
    Finished(Option<PathBuf>),
}

/// Where audio chunks are transcribed. Local keeps audio on this computer.
pub enum TranscriptionBackend {
    Local {
        model_path: PathBuf,
        compute: whisper::ComputePreference,
    },
    Remote {
        provider: ProviderConfig,
    },
}

/// The backend after session start-up (local models load once, up front).
enum Transcription {
    Local(Arc<whisper::Transcriber>),
    Remote { provider: ProviderConfig },
}

struct TranscriptionOptions {
    model: String,
    language: String,
    silence_threshold: u16,
    local_window_seconds: u64,
}

pub struct SessionServices {
    pub transcription: TranscriptionBackend,
    pub analysis_task: TaskConfig,
    pub analysis_provider: ProviderConfig,
    pub vision_settings: VisionConfig,
    pub vision_provider: Option<ProviderConfig>,
    pub profile: AssistantProfile,
}

/// Run until `running` becomes false. Errors are reported as events so the UI
/// can remain responsive and always reset its session controls.
pub async fn run_session(
    settings: MeetingConfig,
    services: SessionServices,
    events: Sender<SessionEvent>,
    mut running: watch::Receiver<bool>,
) {
    if let Err(err) = run(&settings, &services, &events, &mut running).await {
        let _ = events.send(SessionEvent::Error(format!("{err:#}"))).await;
        let _ = events.send(SessionEvent::Finished(None)).await;
    }
}

async fn run(
    settings: &MeetingConfig,
    services: &SessionServices,
    events: &Sender<SessionEvent>,
    running: &mut watch::Receiver<bool>,
) -> Result<()> {
    let SessionServices {
        transcription,
        analysis_task,
        analysis_provider,
        vision_settings,
        vision_provider,
        profile,
    } = services;
    if settings.chunk_seconds == 0 || settings.chunk_seconds > 60 {
        bail!("audio chunk duration must be between 1 and 60 seconds");
    }
    let transcription = match transcription {
        TranscriptionBackend::Local {
            model_path,
            compute,
        } => {
            let _ = events
                .send(SessionEvent::Status("loading local whisper model…".into()))
                .await;
            let model_path = model_path.clone();
            let compute = *compute;
            let transcriber = tokio::task::spawn_blocking(move || {
                whisper::Transcriber::load(&model_path, compute)
            })
            .await
            .context("whisper loading task failed")??;
            Transcription::Local(Arc::new(transcriber))
        }
        TranscriptionBackend::Remote { provider } => {
            if provider.kind != ProviderKind::Openai {
                bail!("remote transcription requires an OpenAI-compatible provider");
            }
            Transcription::Remote {
                provider: provider.clone(),
            }
        }
    };

    let devices = capture_devices(settings)?;
    let bytes_per_chunk = SAMPLE_RATE as usize
        * CHANNELS as usize
        * (BITS_PER_SAMPLE as usize / 8)
        * settings.chunk_seconds as usize;
    let audio = capture_audio(devices.clone(), bytes_per_chunk, running.clone());
    let backend_label = match &transcription {
        Transcription::Local(transcriber) => format!(
            "local whisper ({}, {})",
            settings.whisper_model,
            transcriber.compute_label()
        ),
        Transcription::Remote { .. } => "remote transcription API".to_string(),
    };
    let cadence = match &transcription {
        Transcription::Local(_) => format!(
            "{}s capture stride · {}s rolling window",
            settings.chunk_seconds,
            settings
                .transcription_window_seconds
                .max(settings.chunk_seconds)
        ),
        Transcription::Remote { .. } => format!("{}s uploaded windows", settings.chunk_seconds),
    };
    let transcriptions = transcribe_audio(
        audio,
        transcription,
        TranscriptionOptions {
            model: settings.transcription_model.clone(),
            language: settings.input_language.clone(),
            silence_threshold: settings.silence_threshold,
            local_window_seconds: settings
                .transcription_window_seconds
                .max(settings.chunk_seconds),
        },
        events.clone(),
        running.clone(),
    );

    let _ = events
        .send(SessionEvent::Status(format!(
            "continuous transcription · {backend_label} · {} · {cadence}",
            devices.join(" + "),
        )))
        .await;

    let mut transcript = Vec::new();
    let mut translations = Vec::new();
    let mut notes = Vec::new();
    let mut chunk_index = 0_u32;
    let mut last_screen_chunk = 0_u32;

    loop {
        let first = tokio::select! {
            result = transcriptions.recv() => result.ok(),
            changed = running.changed() => {
                if changed.is_err() || !*running.borrow() { None } else { continue }
            }
        };
        let Some(first) = first else {
            break;
        };
        if !*running.borrow() {
            break;
        }

        // Analysis may be slower than transcription. Drain every transcript
        // already waiting and coach from the newest combined context instead
        // of producing stale suggestions for old audio windows.
        let mut batch = vec![first];
        while let Ok(next) = transcriptions.try_recv() {
            batch.push(next);
        }
        chunk_index += batch.len() as u32;
        transcript.extend(batch.iter().cloned());
        let text = batch.join("\n");

        let recent = recent_transcript(&transcript, 8_000);
        let screen_due = settings.screen_context
            && chunk_index.saturating_sub(last_screen_chunk)
                >= settings.screen_interval_chunks.max(1);
        let captured_image = if screen_due {
            last_screen_chunk = chunk_index;
            match screenshot::capture_png().await {
                Ok(image) => Some(image),
                Err(err) => {
                    let _ = events
                        .send(SessionEvent::Error(format!(
                            "screen context unavailable: {err:#}"
                        )))
                        .await;
                    None
                }
            }
        } else {
            None
        };
        let (image, screen_description) = match (vision_settings.mode.as_str(), captured_image) {
            ("off", _) | (_, None) => (None, None),
            ("proxy", Some(image)) => match vision_provider.as_ref() {
                Some(provider) => match vision::describe_screen(
                    provider,
                    &vision_settings.model,
                    &vision_settings.prompt,
                    image,
                )
                .await
                {
                    Ok(description) => (None, Some(description)),
                    Err(err) => {
                        let _ = events
                            .send(SessionEvent::Error(format!(
                                "vision/OCR proxy failed: {err:#}"
                            )))
                            .await;
                        (None, None)
                    }
                },
                None => {
                    let _ = events
                        .send(SessionEvent::Error(
                            "vision proxy is enabled but its provider is not configured".into(),
                        ))
                        .await;
                    (None, None)
                }
            },
            (_, Some(image)) => (Some(image), None),
        };

        let translation_request = settings.translate.then(|| {
            request(
                analysis_task,
                format!(
                    "Translate the following spoken text into {}. Output only the translation:\n\n{text}",
                    settings.target_language
                ),
                "You are an accurate live interpreter. Preserve meaning, tone, names, and numbers."
                    .into(),
                None,
            )
        });
        let insight_request = (settings.suggestions
            || settings.objection_handling
            || settings.automatic_notes)
            .then(|| {
                request(
                    analysis_task,
                    coaching_prompt(settings, &recent, screen_description.as_deref()),
                    profile.system.clone(),
                    image,
                )
            });

        let translation = async {
            match translation_request {
                Some(request) => complete_chat(analysis_provider, request).await.map(Some),
                None => Ok(None),
            }
        };
        let insight = async {
            match insight_request {
                Some(request) => complete_chat(analysis_provider, request).await.map(Some),
                None => Ok(None),
            }
        };
        let (translation, insight) = tokio::join!(translation, insight);

        match translation {
            Ok(Some(text)) => {
                translations.push(text.clone());
                let _ = events.send(SessionEvent::Translation(text)).await;
            }
            Err(err) => {
                let _ = events
                    .send(SessionEvent::Error(format!("translation failed: {err:#}")))
                    .await;
            }
            Ok(None) => {}
        }
        match insight {
            Ok(Some(text)) => {
                notes.push(text.clone());
                let _ = events.send(SessionEvent::Insight(text)).await;
            }
            Err(err) => {
                let _ = events
                    .send(SessionEvent::Error(format!(
                        "live coaching failed: {err:#}"
                    )))
                    .await;
            }
            Ok(None) => {}
        }
    }

    let _ = events
        .send(SessionEvent::Status("creating session summary…".into()))
        .await;

    let summary = if settings.summary && !transcript.is_empty() {
        let prompt = session_summary_prompt(&notes, &transcript);
        match complete_chat(
            analysis_provider,
            request(analysis_task, prompt, profile.system.clone(), None),
        )
        .await
        {
            Ok(summary) => {
                let _ = events.send(SessionEvent::Summary(summary.clone())).await;
                Some(summary)
            }
            Err(err) => {
                let _ = events
                    .send(SessionEvent::Error(format!("summary failed: {err:#}")))
                    .await;
                None
            }
        }
    } else {
        None
    };

    let path = if settings.save_session && !transcript.is_empty() {
        Some(save_session(
            &transcript,
            &translations,
            &notes,
            summary.as_deref(),
        )?)
    } else {
        None
    };
    let _ = events.send(SessionEvent::Finished(path)).await;
    Ok(())
}

/// Transcribe continuously in a task independent from coaching/translation.
/// This keeps transcript updates flowing while a slower reasoning model is
/// still producing suggestions for a previous window.
fn transcribe_audio(
    audio: async_channel::Receiver<Result<Vec<u8>, String>>,
    backend: Transcription,
    options: TranscriptionOptions,
    events: Sender<SessionEvent>,
    mut running: watch::Receiver<bool>,
) -> async_channel::Receiver<String> {
    let (tx, rx) = async_channel::unbounded();
    tokio::spawn(async move {
        let local_window_bytes = SAMPLE_RATE as usize
            * CHANNELS as usize
            * (BITS_PER_SAMPLE as usize / 8)
            * options.local_window_seconds as usize;
        let mut rolling_audio = VecDeque::<Vec<u8>>::new();
        let mut rolling_bytes = 0_usize;
        let mut previous_window_transcript: Option<String> = None;
        loop {
            let pcm = tokio::select! {
                result = audio.recv() => match result {
                    Ok(Ok(pcm)) => Some(pcm),
                    Ok(Err(message)) => {
                        let _ = events.send(SessionEvent::Error(message)).await;
                        None
                    }
                    Err(_) => None,
                },
                changed = running.changed() => {
                    if changed.is_err() || !*running.borrow() { None } else { continue }
                }
            };
            let Some(pcm) = pcm else { break };
            let silent =
                options.silence_threshold > 0 && pcm_level(&pcm) < options.silence_threshold;
            if matches!(&backend, Transcription::Local(_)) {
                rolling_bytes += pcm.len();
                rolling_audio.push_back(pcm.clone());
                while rolling_bytes > local_window_bytes {
                    let Some(oldest) = rolling_audio.pop_front() else {
                        break;
                    };
                    rolling_bytes = rolling_bytes.saturating_sub(oldest.len());
                }
            }
            if silent {
                previous_window_transcript = None;
                continue;
            }
            let transcribed = match &backend {
                Transcription::Local(transcriber) => {
                    if rolling_bytes < local_window_bytes {
                        continue;
                    }
                    let pcm: Vec<u8> = rolling_audio
                        .iter()
                        .flat_map(|chunk| chunk.iter().copied())
                        .collect();
                    let transcriber = Arc::clone(transcriber);
                    let language = options.language.clone();
                    tokio::task::spawn_blocking(move || transcriber.transcribe(&pcm, &language))
                        .await
                        .unwrap_or_else(|err| Err(anyhow::anyhow!("whisper task failed: {err}")))
                }
                Transcription::Remote { provider } => {
                    transcribe(
                        provider,
                        &options.model,
                        &options.language,
                        pcm_to_wav(&pcm),
                    )
                    .await
                }
            };
            match transcribed {
                Ok(text) if !text.trim().is_empty() => {
                    let raw = text.trim().to_string();
                    let text = if matches!(&backend, Transcription::Local(_)) {
                        let novel = previous_window_transcript.as_deref().map_or_else(
                            || raw.clone(),
                            |previous| whisper::novel_transcript(previous, &raw),
                        );
                        previous_window_transcript = Some(raw);
                        novel
                    } else {
                        raw
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let _ = events.send(SessionEvent::Transcript(text.clone())).await;
                    if tx.send(text).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    let _ = events
                        .send(SessionEvent::Error(format!(
                            "transcription failed: {err:#}"
                        )))
                        .await;
                }
            }
        }
    });
    rx
}

/// Keep capture independent from network latency. The bounded queue absorbs a
/// short spike; when analysis falls behind, new chunks are dropped instead of
/// allowing suggestions to drift minutes behind the live conversation.
fn capture_audio(
    devices: Vec<String>,
    bytes_per_chunk: usize,
    mut running: watch::Receiver<bool>,
) -> async_channel::Receiver<Result<Vec<u8>, String>> {
    let (tx, rx) = async_channel::bounded(3);
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut recorders = Vec::new();
            for device in devices {
                let mut child = recorder(&device)?;
                let stdout = child
                    .stdout
                    .take()
                    .context("parec stdout was unavailable")?;
                recorders.push((child, stdout));
            }
            loop {
                let mut first = vec![0_u8; bytes_per_chunk];
                let mut second = (recorders.len() == 2).then(|| vec![0_u8; bytes_per_chunk]);
                let read = tokio::select! {
                    result = async {
                        if let Some(second) = second.as_mut() {
                            let (first_recorder, second_recorder) = recorders.split_at_mut(1);
                            let (first_result, second_result) = tokio::join!(
                                first_recorder[0].1.read_exact(&mut first),
                                second_recorder[0].1.read_exact(second),
                            );
                            first_result?;
                            second_result?;
                            Ok(())
                        } else {
                            recorders[0].1.read_exact(&mut first).await.map(|_| ())
                        }
                    } => Some(result),
                    changed = running.changed() => {
                        if changed.is_err() || !*running.borrow() { None } else { continue }
                    }
                };
                let Some(read) = read else { break };
                read.context("audio capture stopped")?;
                let pcm = match second {
                    Some(second) => mix_pcm(&first, &second),
                    None => first,
                };
                // A full queue means the AI is slower than real time. Replace
                // the oldest queued block so coaching stays near the present.
                if tx.force_send(Ok(pcm)).is_err() {
                    break;
                }
            }
            for (mut child, _) in recorders {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            Ok(())
        }
        .await;
        if let Err(err) = result {
            let _ = tx.send(Err(format!("{err:#}"))).await;
        }
    });
    rx
}

fn recorder(device: &str) -> Result<tokio::process::Child> {
    Command::new("parec")
        .args([
            "--record",
            "--raw",
            "--format=s16le",
            "--rate=16000",
            "--channels=1",
            &format!("--device={device}"),
            "--client-name=Nexora",
            "--stream-name=Live meeting transcription",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("could not start `parec`; install PulseAudio utilities")
}

fn mix_pcm(first: &[u8], second: &[u8]) -> Vec<u8> {
    first
        .chunks_exact(2)
        .zip(second.chunks_exact(2))
        .flat_map(|(first, second)| {
            let first = i16::from_le_bytes([first[0], first[1]]) as i32;
            let second = i16::from_le_bytes([second[0], second[1]]) as i32;
            let mixed = ((first + second) / 2) as i16;
            mixed.to_le_bytes()
        })
        .collect()
}

fn pcm_level(pcm: &[u8]) -> u16 {
    let mut total = 0_u64;
    let mut samples = 0_u64;
    for sample in pcm.chunks_exact(2) {
        let value = i16::from_le_bytes([sample[0], sample[1]]) as i32;
        total += value.unsigned_abs() as u64;
        samples += 1;
    }
    total.checked_div(samples).unwrap_or(0).min(u16::MAX as u64) as u16
}

fn capture_devices(settings: &MeetingConfig) -> Result<Vec<String>> {
    match settings.audio_source.as_str() {
        "microphone" => Ok(vec!["@DEFAULT_SOURCE@".into()]),
        "system" => Ok(vec!["@DEFAULT_MONITOR@".into()]),
        "both" => Ok(vec!["@DEFAULT_MONITOR@".into(), "@DEFAULT_SOURCE@".into()]),
        "custom" if !settings.audio_device.trim().is_empty() => {
            Ok(vec![settings.audio_device.trim().into()])
        }
        "custom" => bail!("enter a custom audio device in Settings"),
        other => bail!("unknown audio source `{other}`"),
    }
}

async fn transcribe(
    provider: &ProviderConfig,
    model: &str,
    language: &str,
    wav: Vec<u8>,
) -> Result<String> {
    let audio = Part::bytes(wav)
        .file_name("nexora-chunk.wav")
        .mime_str("audio/wav")?;
    let mut form = Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .part("file", audio);
    if !language.trim().is_empty() {
        form = form.text("language", language.trim().to_string());
    }
    let response = reqwest::Client::new()
        .post(format!("{}/audio/transcriptions", provider.base_url()))
        .bearer_auth(provider.resolve_api_key()?)
        .multipart(form)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("transcription API returned {status}: {}", excerpt(&body));
    }
    let value: Value = serde_json::from_str(&body).context("invalid transcription response")?;
    value["text"]
        .as_str()
        .map(ToOwned::to_owned)
        .context("transcription response had no `text` field")
}

fn request(
    task: &TaskConfig,
    prompt: String,
    system: String,
    image: Option<Vec<u8>>,
) -> ChatRequest {
    ChatRequest {
        model: task.model.clone(),
        system: Some(system),
        messages: vec![(Role::User, prompt)],
        image_png: image,
        max_tokens: task.max_tokens,
    }
}

fn coaching_prompt(
    settings: &MeetingConfig,
    transcript: &str,
    screen_description: Option<&str>,
) -> String {
    let mut goals = Vec::new();
    if settings.suggestions {
        goals.push("suggest the best short reply, useful arguments, and relevant information");
    }
    if settings.objection_handling {
        goals.push("identify objections and propose respectful, evidence-based responses");
    }
    if settings.automatic_notes {
        goals.push("capture decisions, facts, questions, and action items as compact notes");
    }
    let mut prompt = format!(
        "You are coaching a conversation as it happens. {}.\n\nEvidence rules:\n- Use only information explicitly present in the transcript or attached screen.\n- A question is not a fact or decision. Keep it as an open question.\n- Do not infer identities, relationships, business domain, intent, policies, location, logistics, costs, approvals, or prior statements.\n- Never turn a suggested reply into a fact, decision, or note.\n- If the fragment does not support useful grounded coaching, output only: Wait for more context.\n\nOtherwise, separate enabled sections with short labels, stay concise, and explicitly mark uncertainty. Do not repeat the transcript. Use the attached screen only when relevant.\n\nRecent transcript:\n{transcript}",
        goals.join("; ")
    );
    if let Some(description) = screen_description {
        prompt.push_str("\n\nScreen context from vision/OCR:\n");
        prompt.push_str(description);
    }
    prompt
}

fn recent_transcript(chunks: &[String], max_chars: usize) -> String {
    let mut selected = Vec::new();
    let mut length = 0;
    for chunk in chunks.iter().rev() {
        if length + chunk.len() > max_chars && !selected.is_empty() {
            break;
        }
        length += chunk.len();
        selected.push(chunk.as_str());
    }
    selected.reverse();
    selected.join("\n")
}

fn session_summary_prompt(notes: &[String], transcript: &[String]) -> String {
    format!(
        "Summarize this session using the grounding rules below.\n- The transcript is the source of truth.\n- Generated notes are untrusted model suggestions, not evidence. Include a note only when the transcript independently supports it.\n- A question is not a decision. A proposed reply is not something a participant actually said.\n- If notes conflict with or go beyond the transcript, discard them.\n- Do not infer identities, intent, domain, policies, location, costs, owners, or action items.\n\nInclude only supported decisions, key points, objections, open questions, and action items with owners when explicitly stated. Say when the available transcript is insufficient.\n\nGenerated rolling notes (untrusted):\n{}\n\nRecent transcript (source of truth):\n{}",
        recent_transcript(notes, 20_000),
        recent_transcript(transcript, 40_000),
    )
}

fn pcm_to_wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * BITS_PER_SAMPLE as u32 / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

fn save_session(
    transcript: &[String],
    translations: &[String],
    notes: &[String],
    summary: Option<&str>,
) -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("nexora")
        .join("sessions");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("session-{id}.md"));
    let mut document = format!(
        "# Nexora Session {id}\n\n## Transcript\n\n{}\n",
        transcript.join("\n\n")
    );
    if !translations.is_empty() {
        document.push_str(&format!(
            "\n## Translation\n\n{}\n",
            translations.join("\n\n")
        ));
    }
    if !notes.is_empty() {
        document.push_str(&format!(
            "\n## Live Coaching and Notes\n\n{}\n",
            notes.join("\n\n")
        ));
    }
    if let Some(summary) = summary {
        document.push_str(&format!("\n## Summary\n\n{summary}\n"));
    }
    write_private(&path, document.as_bytes())?;
    Ok(path)
}

fn write_private(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents)?;
    Ok(())
}

fn excerpt(text: &str) -> String {
    text.chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_contains_pcm_size() {
        let wav = pcm_to_wav(&[0; 320]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 320);
        assert_eq!(wav.len(), 364);
    }

    #[test]
    fn recent_transcript_keeps_latest_chunks() {
        let chunks = vec!["old text".into(), "middle".into(), "latest".into()];
        assert_eq!(recent_transcript(&chunks, 13), "middle\nlatest");
    }

    #[test]
    fn custom_capture_device_must_be_set() {
        let settings = MeetingConfig {
            audio_source: "custom".into(),
            ..MeetingConfig::default()
        };
        assert!(capture_devices(&settings).is_err());
    }

    #[test]
    fn mixes_two_pcm_streams_without_clipping() {
        let first = 10_000_i16.to_le_bytes();
        let second = 20_000_i16.to_le_bytes();
        let mixed = mix_pcm(&first, &second);
        assert_eq!(i16::from_le_bytes(mixed.try_into().unwrap()), 15_000);
    }

    #[test]
    fn silence_gate_measures_average_pcm_amplitude() {
        let pcm: Vec<u8> = [100_i16, -300, 200, -200]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        assert_eq!(pcm_level(&pcm), 200);
        assert_eq!(pcm_level(&[]), 0);
    }

    #[test]
    fn coaching_prompt_treats_questions_as_questions_not_decisions() {
        let prompt = coaching_prompt(
            &MeetingConfig::default(),
            "E para os caras ficarem no Brasil?",
            None,
        );

        assert!(prompt.contains("A question is not a fact or decision"));
        assert!(prompt.contains("Do not infer"));
        assert!(prompt.contains("Wait for more context"));
    }

    #[test]
    fn summary_uses_transcript_as_truth_not_generated_notes() {
        let prompt = session_summary_prompt(
            &["Decision: the team is in Brazil".into()],
            &["E para os caras ficarem no Brasil?".into()],
        );

        assert!(prompt.contains("The transcript is the source of truth"));
        assert!(prompt.contains("Generated notes are untrusted"));
        assert!(prompt.contains("A question is not a decision"));
    }
}
