//! Live meeting session lifecycle: starting/stopping the background session
//! task, routing its events, and selecting transcript context for questions.

use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;

use crate::conversation::Role;
use crate::hidden::HiddenState;
use crate::meeting::{self, SessionEvent};
use crate::runtime;

use super::overlay::Overlay;

/// Attach live-meeting context to the outgoing question. Public so developer
/// tools (src/bin/) can send questions in exactly the format the app uses.
pub fn append_meeting_transcript_context(
    messages: &mut [(Role, String)],
    transcript: &[String],
    max_chars: usize,
) {
    let Some((_, text)) = messages.last_mut() else {
        return;
    };
    let question = text.clone();
    if let Some(context) = meeting_transcript_context(&question, transcript, max_chars) {
        text.push_str(
            "\n\nLive session context selected from the transcript. Treat transcription as potentially imperfect; ground claims about the conversation in this evidence. If the question goes beyond what was said, answer it from general knowledge and say that the audio does not cover it:\n",
        );
        text.push_str(&context);
    }
}

fn meeting_transcript_context(
    question: &str,
    transcript: &[String],
    max_chars: usize,
) -> Option<String> {
    if transcript.is_empty() || max_chars < 200 {
        return None;
    }

    let label_budget = 80;
    let content_budget = max_chars.saturating_sub(label_budget);
    let recent_budget = content_budget * 3 / 5;
    let relevant_budget = content_budget.saturating_sub(recent_budget);

    let mut recent_start = transcript.len();
    let mut recent_chars = 0;
    while recent_start > 0 {
        let next = transcript[recent_start - 1].chars().count() + 1;
        if recent_chars + next > recent_budget && recent_start < transcript.len() {
            break;
        }
        recent_start -= 1;
        recent_chars += next;
    }

    let query_terms = meaningful_terms(question);
    let mut scored: Vec<(usize, usize)> = transcript[..recent_start]
        .iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let chunk_terms = meaningful_terms(chunk);
            let score = query_terms
                .iter()
                .filter(|term| chunk_terms.contains(*term))
                .map(|term| 1 + term.chars().count().min(12) / 4)
                .sum();
            (score > 0).then_some((score, index))
        })
        .collect();
    scored.sort_unstable_by(|(left_score, left_index), (right_score, right_index)| {
        right_score
            .cmp(left_score)
            .then_with(|| right_index.cmp(left_index))
    });

    let mut relevant_indices = BTreeSet::new();
    let mut relevant_chars = 0;
    for (_, anchor) in scored {
        let neighbors = [
            Some(anchor),
            anchor.checked_sub(1),
            (anchor + 1 < recent_start).then_some(anchor + 1),
        ];
        for index in neighbors.into_iter().flatten() {
            let size = transcript[index].chars().count() + 1;
            if relevant_chars + size <= relevant_budget && relevant_indices.insert(index) {
                relevant_chars += size;
            }
        }
        if relevant_chars >= relevant_budget {
            break;
        }
    }

    let mut context = String::new();
    if !relevant_indices.is_empty() {
        context.push_str("Most relevant earlier speech:\n");
        for index in relevant_indices {
            context.push_str(&transcript[index]);
            context.push('\n');
        }
    }
    context.push_str("Most recent speech:\n");
    let recent = transcript[recent_start..].join("\n");
    context.push_str(&tail_chars(&recent, recent_budget));
    Some(context)
}

fn meaningful_terms(text: &str) -> HashSet<String> {
    const STOPWORDS: &[&str] = &[
        "a", "as", "ao", "com", "da", "das", "de", "do", "dos", "e", "em", "era", "foi", "o", "os",
        "para", "pela", "pelo", "por", "qual", "que", "quem", "the", "what", "when", "where",
        "which", "who", "why",
    ];
    let mut terms = HashSet::new();
    let mut current = String::new();
    for character in text.chars().chain(std::iter::once(' ')) {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            if current.chars().count() >= 3 && !STOPWORDS.contains(&current.as_str()) {
                terms.insert(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    terms
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(max_chars)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeetingEventSurface {
    Status,
    Live,
}

fn meeting_event_surface(event: &SessionEvent) -> MeetingEventSurface {
    match event {
        SessionEvent::Status(_) | SessionEvent::Finished(_) => MeetingEventSurface::Status,
        SessionEvent::Transcript(_)
        | SessionEvent::Translation(_)
        | SessionEvent::Insight(_)
        | SessionEvent::Summary(_)
        | SessionEvent::Error(_) => MeetingEventSurface::Live,
    }
}

fn visible_coaching_text(text: &str) -> Option<&str> {
    let text = text.trim();
    let normalized = text
        .trim_matches(|character| matches!(character, '*' | '_'))
        .trim();
    (!normalized.eq_ignore_ascii_case("wait for more context.")
        && !normalized.eq_ignore_ascii_case("wait for more context"))
    .then_some(text)
}

/// Questions are sent immediately with the transcript available at Enter.
/// Only a live session that has produced no transcript at all (model still
/// warming up, opening silence) briefly waits for the first line.
pub(super) fn should_wait_for_first_transcript(
    meeting_active: bool,
    transcript_len: usize,
) -> bool {
    meeting_active && transcript_len == 0
}

/// Resolved services a meeting session needs, computed from the current
/// config before the session task is spawned.
struct ResolvedMeetingServices {
    settings: crate::config::MeetingConfig,
    transcription: meeting::TranscriptionBackend,
    task: crate::config::TaskConfig,
    analysis_provider: crate::config::ProviderConfig,
    vision_settings: crate::config::VisionConfig,
    vision_provider: Option<crate::config::ProviderConfig>,
    profile: crate::config::AssistantProfile,
}

fn resolve_meeting_services(
    config: &crate::config::Config,
) -> anyhow::Result<ResolvedMeetingServices> {
    let settings = config.meeting.clone();
    let vision_settings = config.vision.clone();
    let transcription = if settings.transcription_backend == "local" {
        let model_path = crate::whisper::model_path(&settings.whisper_model);
        if model_path.exists() {
            crate::whisper::ComputePreference::from_config(&settings.transcription_compute).map(
                |compute| meeting::TranscriptionBackend::Local {
                    model_path,
                    compute,
                },
            )
        } else {
            Err(anyhow::anyhow!(
                "local whisper model `{}` is not downloaded — open Settings → Live meeting to download it, or switch transcription to remote",
                settings.whisper_model
            ))
        }
    } else {
        config
            .providers
            .get(&settings.transcription_provider)
            .cloned()
            .map(|provider| meeting::TranscriptionBackend::Remote { provider })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "transcription provider `{}` is not configured",
                    settings.transcription_provider
                )
            })
    };
    let analysis = config
        .task(&settings.analysis_task)
        .and_then(|task| Ok((task.clone(), config.provider_for(task)?.clone())));
    let profile = config.profile(&settings.profile);
    let vision_provider = if settings.screen_context && vision_settings.mode == "proxy" {
        config
            .provider(&vision_settings.provider)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "vision provider `{}` is not configured",
                    vision_settings.provider
                )
            })
            .map(Some)
    } else {
        Ok(None)
    };
    match (transcription, analysis, vision_provider, profile) {
        (Ok(transcription), Ok((task, provider)), Ok(vision_provider), Ok(profile)) => {
            Ok(ResolvedMeetingServices {
                settings,
                transcription,
                task,
                analysis_provider: provider,
                vision_settings,
                vision_provider,
                profile,
            })
        }
        (Err(err), _, _, _) | (_, Err(err), _, _) | (_, _, Err(err), _) | (_, _, _, Err(err)) => {
            Err(err)
        }
    }
}

impl Overlay {
    pub(super) fn start_meeting(self: &Rc<Self>) {
        if self.meeting_stop.borrow().is_some() {
            return;
        }
        let resolved = resolve_meeting_services(&self.config.borrow());
        let resolved = match resolved {
            Ok(value) => value,
            Err(err) => {
                self.show_system_line(&format!("meeting cannot start: {err:#}"));
                self.meeting_button.set_active(false);
                return;
            }
        };

        if resolved.settings.screen_context && *self.hidden_state.borrow() != HiddenState::Active {
            self.show_system_line(
                "warning: screen context is enabled, but this compositor cannot confirm that the overlay is excluded from capture",
            );
        }
        self.stack.set_visible_child_name("chat");
        self.gear.set_active(false);
        self.live_response.buffer().set_text("");
        self.show_live_line("Session", "Live assistant started", "meeting");

        self.launch_meeting_session(resolved);
    }

    fn launch_meeting_session(self: &Rc<Self>, resolved: ResolvedMeetingServices) {
        self.meeting_transcript.borrow_mut().clear();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(true);
        *self.meeting_stop.borrow_mut() = Some(stop_tx);
        runtime().spawn(meeting::run_session(
            resolved.settings,
            meeting::SessionServices {
                transcription: resolved.transcription,
                analysis_task: resolved.task,
                analysis_provider: resolved.analysis_provider,
                vision_settings: resolved.vision_settings,
                vision_provider: resolved.vision_provider,
                profile: resolved.profile,
            },
            events_tx,
            stop_rx,
        ));

        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = events_rx.recv().await {
                let finished = matches!(event, SessionEvent::Finished(_));
                this.handle_meeting_event(event);
                if finished {
                    break;
                }
            }
        });
    }

    pub(super) fn stop_meeting(&self) {
        if let Some(stop) = self.meeting_stop.borrow().as_ref() {
            let _ = stop.send(false);
            self.set_status("stopping meeting and preparing summary…");
        }
    }

    pub fn start_session(&self) {
        if self.meeting_stop.borrow().is_none() {
            self.meeting_button.set_active(true);
        }
    }

    pub fn stop_session(&self) {
        if self.meeting_stop.borrow().is_some() {
            self.meeting_button.set_active(false);
        }
    }

    pub(super) fn handle_meeting_event(&self, event: SessionEvent) {
        if meeting_event_surface(&event) == MeetingEventSurface::Status {
            match event {
                SessionEvent::Status(text) => self.set_status(&text),
                SessionEvent::Finished(path) => {
                    self.meeting_stop.borrow_mut().take();
                    self.meeting_button.set_active(false);
                    match path {
                        Some(path) => {
                            self.set_status(&format!("session saved to {}", path.display()))
                        }
                        None => self.set_status("meeting finished"),
                    }
                }
                _ => unreachable!("status surface only contains status lifecycle events"),
            }
            return;
        }

        match event {
            SessionEvent::Transcript(text) => {
                // Keep a bounded rolling transcript so a typed question can use
                // it as context without growing without bound.
                let mut transcript = self.meeting_transcript.borrow_mut();
                transcript.push(text.clone());
                drop(transcript);
                self.show_live_line("Transcript", &text, "meeting")
            }
            SessionEvent::Translation(text) => {
                self.show_live_line("Translation", &text, "translation")
            }
            SessionEvent::Insight(text) => {
                if let Some(text) = visible_coaching_text(&text) {
                    self.show_live_line("Live coach", text, "insight");
                } else {
                    self.set_status("listening for enough context…");
                }
            }
            SessionEvent::Summary(text) => self.show_live_line("Session summary", &text, "summary"),
            SessionEvent::Error(text) => {
                self.show_live_line("Session issue", &text, "dim");
            }
            SessionEvent::Status(_) | SessionEvent::Finished(_) => {
                unreachable!("lifecycle events returned above")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_question_includes_transcript_after_session_finishes() {
        let mut messages = vec![(Role::User, "sobre o que eles tao conversando?".into())];
        let transcript = vec!["Qual que é a pessoa?".into()];

        append_meeting_transcript_context(&mut messages, &transcript, 6_000);

        assert!(messages[0].1.contains("Qual que é a pessoa?"));
    }

    #[test]
    fn manual_question_waits_for_first_transcript_while_capture_is_active() {
        assert!(should_wait_for_first_transcript(true, 0));
        assert!(!should_wait_for_first_transcript(true, 1));
        assert!(!should_wait_for_first_transcript(false, 0));
    }

    #[test]
    fn live_session_content_is_routed_away_from_chat_history() {
        assert_eq!(
            meeting_event_surface(&SessionEvent::Transcript("speech".into())),
            MeetingEventSurface::Live
        );
        assert_eq!(
            meeting_event_surface(&SessionEvent::Insight("suggestion".into())),
            MeetingEventSurface::Live
        );
        assert_eq!(
            meeting_event_surface(&SessionEvent::Summary("summary".into())),
            MeetingEventSurface::Live
        );
    }

    #[test]
    fn empty_live_coach_placeholder_is_not_presented() {
        assert_eq!(visible_coaching_text("Wait for more context."), None);
        assert_eq!(visible_coaching_text("  wait for more context.  "), None);
        assert_eq!(
            visible_coaching_text("Ask whether the deadline is flexible."),
            Some("Ask whether the deadline is flexible.")
        );
    }

    #[test]
    fn manual_question_sends_instantly_once_any_transcript_exists() {
        assert!(!should_wait_for_first_transcript(true, 3));
        assert!(!should_wait_for_first_transcript(false, 3));
    }

    #[test]
    fn manual_question_retrieves_relevant_context_from_early_in_a_long_session() {
        let mut transcript = vec![
            "Marina explicou que a solução proposta usa filas persistentes chamadas Aurora.".into(),
        ];
        transcript
            .extend((0..250).map(|index| format!("Discussão recente sem relação número {index}.")));
        let mut messages = vec![(
            Role::User,
            "Qual era o nome das filas persistentes mencionadas pela Marina?".into(),
        )];

        append_meeting_transcript_context(&mut messages, &transcript, 6_000);

        assert!(messages[0].1.contains("Aurora"));
        assert!(messages[0].1.contains("Most relevant earlier speech"));
    }
}
