//! Opt-in live meeting pipeline: Pulse/PipeWire capture, transcription,
//! translation, coaching, screen context, notes, and final summary.

mod audio;
mod corrections;
mod persistence;
mod summary;

use std::path::PathBuf;

use anyhow::{Result, bail};
use async_channel::{Receiver, Sender};
use tokio::sync::watch;

use crate::config::{AssistantProfile, MeetingConfig, ProviderConfig, TaskConfig, VisionConfig};
use crate::providers::complete_chat;
use crate::screenshot;
use crate::vision;
use crate::whisper;

pub use corrections::apply_corrections;

use audio::{
    bytes_per_chunk, capture_audio, capture_devices, describe_pipeline, load_transcription_backend,
    transcribe_audio, transcription_options,
};
use summary::{coaching_prompt, recent_transcript, request, session_summary_prompt};

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

pub struct SessionServices {
    pub transcription: TranscriptionBackend,
    pub analysis_task: TaskConfig,
    pub analysis_provider: ProviderConfig,
    pub vision_settings: VisionConfig,
    pub vision_provider: Option<ProviderConfig>,
    pub profile: AssistantProfile,
}

/// Bundles the parts of `SessionServices` and `MeetingConfig` that every
/// per-batch analysis step needs, so helper signatures don't have to thread
/// each of them through separately.
struct AnalysisContext<'a> {
    settings: &'a MeetingConfig,
    task: &'a TaskConfig,
    provider: &'a ProviderConfig,
    profile: &'a AssistantProfile,
    vision_settings: &'a VisionConfig,
    vision_provider: &'a Option<ProviderConfig>,
}

/// Transcript/translations/notes accumulated as the session progresses, plus
/// the screen-context cadence tracker.
#[derive(Default)]
struct SessionState {
    transcript: Vec<String>,
    translations: Vec<String>,
    notes: Vec<String>,
    chunk_index: u32,
    last_screen_chunk: u32,
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
    let ctx = AnalysisContext {
        settings,
        task: analysis_task,
        provider: analysis_provider,
        profile,
        vision_settings,
        vision_provider,
    };

    let transcription = load_transcription_backend(transcription, events).await?;
    let devices = capture_devices(settings)?;
    let audio = capture_audio(devices.clone(), bytes_per_chunk(settings), running.clone());
    let (backend_label, cadence) = describe_pipeline(&transcription, settings);
    let transcriptions = transcribe_audio(
        audio,
        transcription,
        transcription_options(settings),
        events.clone(),
        running.clone(),
    );
    let _ = events
        .send(SessionEvent::Status(format!(
            "continuous transcription · {backend_label} · {} · {cadence}",
            devices.join(" + "),
        )))
        .await;

    let mut state = SessionState::default();
    run_analysis_loop(&ctx, &transcriptions, events, running, &mut state).await;
    finalize_session(&ctx, events, state).await
}

/// Wait for the next batch of transcript lines, draining everything already
/// queued so a slow analysis pass never coaches from a stale window.
async fn next_batch(
    transcriptions: &Receiver<String>,
    running: &mut watch::Receiver<bool>,
) -> Option<Vec<String>> {
    let first = 'select: loop {
        let first = tokio::select! {
            result = transcriptions.recv() => result.ok(),
            changed = running.changed() => {
                if changed.is_err() || !*running.borrow() { None } else { continue 'select; }
            }
        };
        break 'select first;
    };
    let first = first?;
    let mut batch = vec![first];
    while let Ok(next) = transcriptions.try_recv() {
        batch.push(next);
    }
    Some(batch)
}

async fn run_analysis_loop(
    ctx: &AnalysisContext<'_>,
    transcriptions: &Receiver<String>,
    events: &Sender<SessionEvent>,
    running: &mut watch::Receiver<bool>,
    state: &mut SessionState,
) {
    loop {
        let Some(batch) = next_batch(transcriptions, running).await else {
            break;
        };
        if !*running.borrow() {
            break;
        }
        state.chunk_index += batch.len() as u32;
        state.transcript.extend(batch.iter().cloned());
        let text = batch.join("\n");
        process_batch(ctx, events, state, &text).await;
    }
}

async fn process_batch(
    ctx: &AnalysisContext<'_>,
    events: &Sender<SessionEvent>,
    state: &mut SessionState,
    text: &str,
) {
    let recent = recent_transcript(&state.transcript, 8_000);
    let (image, screen_description) = capture_screen_context(ctx, events, state).await;
    run_translation_and_insight(
        ctx,
        events,
        state,
        text,
        &recent,
        screen_description.as_deref(),
        image,
    )
    .await;
}

/// Capture a screenshot when it's due, then resolve it into either a direct
/// image attachment or a vision-model text description, per `vision.mode`.
async fn capture_screen_context(
    ctx: &AnalysisContext<'_>,
    events: &Sender<SessionEvent>,
    state: &mut SessionState,
) -> (Option<Vec<u8>>, Option<String>) {
    let screen_due = ctx.settings.screen_context
        && state.chunk_index.saturating_sub(state.last_screen_chunk)
            >= ctx.settings.screen_interval_chunks.max(1);
    let captured_image = if screen_due {
        state.last_screen_chunk = state.chunk_index;
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
    match (ctx.vision_settings.mode.as_str(), captured_image) {
        ("off", _) | (_, None) => (None, None),
        ("proxy", Some(image)) => match ctx.vision_provider.as_ref() {
            Some(provider) => match vision::describe_screen(
                provider,
                &ctx.vision_settings.model,
                &ctx.vision_settings.prompt,
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
    }
}

/// Run the translation and coaching requests for one batch concurrently, and
/// dispatch their results (or failures) as session events.
async fn run_translation_and_insight(
    ctx: &AnalysisContext<'_>,
    events: &Sender<SessionEvent>,
    state: &mut SessionState,
    text: &str,
    recent: &str,
    screen_description: Option<&str>,
    image: Option<Vec<u8>>,
) {
    let translation_request = ctx.settings.translate.then(|| {
        request(
            ctx.task,
            format!(
                "Translate the following spoken text into {}. Output only the translation:\n\n{text}",
                ctx.settings.target_language
            ),
            "You are an accurate live interpreter. Preserve meaning, tone, names, and numbers."
                .into(),
            None,
        )
    });
    let insight_request = (ctx.settings.suggestions
        || ctx.settings.objection_handling
        || ctx.settings.automatic_notes)
        .then(|| {
            request(
                ctx.task,
                coaching_prompt(ctx.settings, recent, screen_description),
                ctx.profile.system.clone(),
                image,
            )
        });

    let translation = async {
        match translation_request {
            Some(request) => complete_chat(ctx.provider, request).await.map(Some),
            None => Ok(None),
        }
    };
    let insight = async {
        match insight_request {
            Some(request) => complete_chat(ctx.provider, request).await.map(Some),
            None => Ok(None),
        }
    };
    let (translation, insight) = tokio::join!(translation, insight);

    match translation {
        Ok(Some(text)) => {
            state.translations.push(text.clone());
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
            state.notes.push(text.clone());
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

async fn finalize_session(
    ctx: &AnalysisContext<'_>,
    events: &Sender<SessionEvent>,
    state: SessionState,
) -> Result<()> {
    let _ = events
        .send(SessionEvent::Status("creating session summary…".into()))
        .await;

    let summary = if ctx.settings.summary && !state.transcript.is_empty() {
        let prompt = session_summary_prompt(&state.notes, &state.transcript);
        match complete_chat(
            ctx.provider,
            request(ctx.task, prompt, ctx.profile.system.clone(), None),
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

    let path = if ctx.settings.save_session && !state.transcript.is_empty() {
        Some(persistence::save_session(
            &state.transcript,
            &state.translations,
            &state.notes,
            summary.as_deref(),
        )?)
    } else {
        None
    };
    let _ = events.send(SessionEvent::Finished(path)).await;
    Ok(())
}
