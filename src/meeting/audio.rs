//! Continuous audio capture (PulseAudio/PipeWire via `parec`) and its
//! transcription task, independent from coaching/translation so transcript
//! updates keep flowing while a slower reasoning model catches up.

use std::collections::{BTreeMap, VecDeque};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use reqwest::multipart::{Form, Part};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

use super::corrections::apply_corrections;
use super::persistence::{BITS_PER_SAMPLE, CHANNELS, SAMPLE_RATE, excerpt, pcm_to_wav};
use super::{SessionEvent, TranscriptionBackend};
use crate::config::{MeetingConfig, ProviderConfig, ProviderKind};
use crate::whisper;

/// The backend after session start-up (local models load once, up front).
pub(super) enum Transcription {
    Local(Arc<whisper::Transcriber>),
    Remote { provider: ProviderConfig },
}

pub(super) struct TranscriptionOptions {
    pub(super) model: String,
    pub(super) language: String,
    pub(super) silence_threshold: u16,
    pub(super) local_window_seconds: u64,
    pub(super) corrections: BTreeMap<String, String>,
}

pub(super) async fn load_transcription_backend(
    backend: &TranscriptionBackend,
    events: &Sender<SessionEvent>,
) -> Result<Transcription> {
    match backend {
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
            Ok(Transcription::Local(Arc::new(transcriber)))
        }
        TranscriptionBackend::Remote { provider } => {
            if provider.kind != ProviderKind::Openai {
                bail!("remote transcription requires an OpenAI-compatible provider");
            }
            Ok(Transcription::Remote {
                provider: provider.clone(),
            })
        }
    }
}

/// Human-readable backend name and capture/window cadence for the status line.
pub(super) fn describe_pipeline(
    transcription: &Transcription,
    settings: &MeetingConfig,
) -> (String, String) {
    let backend_label = match transcription {
        Transcription::Local(transcriber) => format!(
            "local whisper ({}, {})",
            settings.whisper_model,
            transcriber.compute_label()
        ),
        Transcription::Remote { .. } => "remote transcription API".to_string(),
    };
    let cadence = match transcription {
        Transcription::Local(_) => format!(
            "{}s capture stride · {}s rolling window",
            settings.chunk_seconds,
            settings
                .transcription_window_seconds
                .max(settings.chunk_seconds)
        ),
        Transcription::Remote { .. } => format!("{}s uploaded windows", settings.chunk_seconds),
    };
    (backend_label, cadence)
}

pub(super) fn transcription_options(settings: &MeetingConfig) -> TranscriptionOptions {
    TranscriptionOptions {
        model: settings.transcription_model.clone(),
        language: settings.input_language.clone(),
        silence_threshold: settings.silence_threshold,
        local_window_seconds: settings
            .transcription_window_seconds
            .max(settings.chunk_seconds),
        corrections: settings.corrections.clone(),
    }
}

pub(super) fn bytes_per_chunk(settings: &MeetingConfig) -> usize {
    SAMPLE_RATE as usize
        * CHANNELS as usize
        * (BITS_PER_SAMPLE as usize / 8)
        * settings.chunk_seconds as usize
}

/// Transcribe continuously in a task independent from coaching/translation.
/// This keeps transcript updates flowing while a slower reasoning model is
/// still producing suggestions for a previous window.
pub(super) fn transcribe_audio(
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
                    let text = apply_corrections(&text, &options.corrections);
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
pub(super) fn capture_audio(
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

pub(super) fn capture_devices(settings: &MeetingConfig) -> Result<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
