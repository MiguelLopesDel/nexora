//! Chat rendering, the ask/answer streaming loop, and the text-tag setup
//! shared by the chat and live views.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

use crate::config::ProviderConfig;
use crate::conversation::Role;
use crate::providers::{ChatRequest, StreamEvent, stream_chat};
use crate::runtime;
use crate::vision;

use super::overlay::Overlay;
use super::session::{append_meeting_transcript_context, should_wait_for_first_transcript};

pub(super) fn install_tags(view: &gtk::TextView) {
    let buffer = view.buffer();
    let role = gtk::TextTag::builder()
        .name("role")
        .weight(700)
        .foreground("#b7c4ff")
        .build();
    let dim = gtk::TextTag::builder()
        .name("dim")
        .foreground("#8b93a7")
        .build();
    let meeting = gtk::TextTag::builder()
        .name("meeting")
        .weight(700)
        .foreground("#8fd9a0")
        .build();
    let translation = gtk::TextTag::builder()
        .name("translation")
        .weight(700)
        .foreground("#8ed9e8")
        .build();
    let insight = gtk::TextTag::builder()
        .name("insight")
        .weight(700)
        .foreground("#d5b7ff")
        .build();
    let summary = gtk::TextTag::builder()
        .name("summary")
        .weight(700)
        .foreground("#ffd18e")
        .build();
    buffer.tag_table().add(&role);
    buffer.tag_table().add(&dim);
    buffer.tag_table().add(&meeting);
    buffer.tag_table().add(&translation);
    buffer.tag_table().add(&insight);
    buffer.tag_table().add(&summary);
}

impl Overlay {
    /// Send a prompt in the current conversation, optionally attaching a shot.
    pub fn ask(self: &Rc<Self>, prompt: String, attach_screen: bool, task_name: String) {
        if self.busy.get() {
            self.set_status("still answering — wait for the current response");
            return;
        }
        self.busy.set(true);
        self.live_button.set_active(false);
        self.stack.set_visible_child_name("chat");
        self.gear.set_active(false);

        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            this.run_ask(prompt, attach_screen, task_name).await;
            this.busy.set(false);
        });
    }

    async fn run_ask(self: &Rc<Self>, prompt: String, attach_screen: bool, task_name: String) {
        // Capture first so a failure doesn't leave a half-formed turn.
        let image = if attach_screen {
            self.set_status("capturing screen…");
            match self.capture_avoiding_self().await {
                Ok(png) => Some(png),
                Err(err) => {
                    self.set_status(&format!("screenshot failed: {err:#}"));
                    return;
                }
            }
        } else {
            None
        };
        self.present();

        let Some((task, provider)) = self.resolve_task_and_provider(&task_name) else {
            return;
        };
        let Some((image, screen_description)) = self.resolve_image_and_description(image).await
        else {
            return;
        };

        self.wait_for_first_meeting_transcript().await;
        // A live session without any transcript yet (model warming up, silent
        // stretch) must not block the user: send the question anyway and say
        // clearly that no meeting context was attached.
        let missing_live_context =
            self.meeting_stop.borrow().is_some() && self.meeting_transcript.borrow().is_empty();
        if missing_live_context {
            self.show_system_line(
                "No speech has been transcribed yet, so this question was sent without live meeting context.",
            );
        }

        self.conversation
            .borrow_mut()
            .push_user(prompt, image.is_some());
        self.render_conversation();
        self.begin_assistant_line();
        self.set_status(&format!("{} · {}", task.provider, task.model));

        let messages = self.build_api_messages(screen_description);
        let request = ChatRequest::new(&task, messages, image);
        self.stream_answer(provider, request).await;
    }

    /// Resolve the task's provider/model, reporting a config error in place.
    fn resolve_task_and_provider(
        &self,
        task_name: &str,
    ) -> Option<(crate::config::TaskConfig, ProviderConfig)> {
        let outcome = {
            let config = self.config.borrow();
            config
                .task(task_name)
                .and_then(|task| Ok((task.clone(), config.provider_for(task)?.clone())))
        };
        match outcome {
            Ok(pair) => Some(pair),
            Err(err) => {
                self.set_status("not configured");
                self.show_system_line(&format!(
                    "{err:#}\nOpen ⚙ Settings (or run `nexora config init`) to set a provider."
                ));
                None
            }
        }
    }

    /// Resolve the attached screenshot into either a direct image or, when
    /// vision proxy mode is on, a text description. `None` means an error was
    /// already reported and the caller should stop.
    async fn resolve_image_and_description(
        &self,
        image: Option<Vec<u8>>,
    ) -> Option<(Option<Vec<u8>>, Option<String>)> {
        let mut image = image;
        let mut screen_description = None;
        if image.is_some() {
            let vision_config = self.config.borrow().vision.clone();
            match vision_config.mode.as_str() {
                "off" => image = None,
                "proxy" => {
                    let vision_provider = self
                        .config
                        .borrow()
                        .provider(&vision_config.provider)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "vision provider `{}` is not configured",
                                vision_config.provider
                            )
                        });
                    let vision_provider = match vision_provider {
                        Ok(provider) => provider,
                        Err(err) => {
                            self.set_status("vision proxy not configured");
                            self.show_system_line(&format!("{err:#}"));
                            return None;
                        }
                    };
                    self.set_status("describing screen with vision/OCR…");
                    match vision::describe_screen(
                        &vision_provider,
                        &vision_config.model,
                        &vision_config.prompt,
                        image.take().expect("screen image was checked"),
                    )
                    .await
                    {
                        Ok(description) => screen_description = Some(description),
                        Err(err) => {
                            self.set_status("vision/OCR failed");
                            self.show_system_line(&format!("vision/OCR failed: {err:#}"));
                            return None;
                        }
                    }
                }
                _ => {}
            }
        }
        Some((image, screen_description))
    }

    fn build_api_messages(&self, screen_description: Option<String>) -> Vec<(Role, String)> {
        let mut messages = self.conversation.borrow().api_messages();
        let context_chars = self
            .config
            .borrow()
            .meeting
            .question_context_chars
            .clamp(2_000, 64_000);
        append_meeting_transcript_context(
            &mut messages,
            &self.meeting_transcript.borrow(),
            context_chars,
        );
        if let Some(description) = screen_description
            && let Some((_, text)) = messages.last_mut()
        {
            text.push_str("\n\nScreen context from vision/OCR:\n");
            text.push_str(&description);
        }
        messages
    }

    async fn stream_answer(&self, provider: ProviderConfig, request: ChatRequest) {
        let (tx, rx) = async_channel::unbounded::<StreamEvent>();
        runtime().spawn(async move { stream_chat(&provider, request, tx).await });

        let mut answer = String::new();
        while let Ok(event) = rx.recv().await {
            match event {
                StreamEvent::Delta(text) => {
                    answer.push_str(&text);
                    self.append_text(&text);
                }
                StreamEvent::Done => break,
                StreamEvent::Error(message) => {
                    // Drop the user turn so history stays valid for a retry.
                    self.conversation.borrow_mut().turns.pop();
                    self.render_conversation();
                    self.show_system_line(&format!("error: {message}"));
                    self.set_status("error");
                    return;
                }
            }
        }

        let mut conversation = self.conversation.borrow_mut();
        conversation.push_assistant(answer);
        if let Err(err) = conversation.save() {
            eprintln!("nexora: could not save history: {err:#}");
        }
        drop(conversation);
        self.render_conversation();
        self.set_status("");
    }

    async fn wait_for_first_meeting_transcript(&self) {
        let meeting_active = self.meeting_stop.borrow().is_some();
        let current_len = self.meeting_transcript.borrow().len();
        if !should_wait_for_first_transcript(meeting_active, current_len) {
            return;
        }

        let timeout_ms = self
            .config
            .borrow()
            .meeting
            .question_context_wait_ms
            .min(5_000);
        if timeout_ms == 0 {
            return;
        }
        let timeout = Duration::from_millis(timeout_ms);
        let deadline = Instant::now() + timeout;
        self.set_status("waiting for the first transcript line…");
        while Instant::now() < deadline {
            if !self.meeting_transcript.borrow().is_empty() || self.meeting_stop.borrow().is_none()
            {
                break;
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
    }

    async fn capture_avoiding_self(&self) -> anyhow::Result<Vec<u8>> {
        let must_hide = self.window.is_visible()
            && *self.hidden_state.borrow() != crate::hidden::HiddenState::Active;
        if must_hide {
            self.window.set_visible(false);
            glib::timeout_future(Duration::from_millis(300)).await;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime().spawn(async move {
            let _ = tx.send(crate::screenshot::capture_png().await);
        });
        let result = rx
            .await
            .map_err(|_| anyhow::anyhow!("capture task dropped"))?;
        if must_hide {
            self.window.set_visible(true);
        }
        result
    }

    // --- Conversation rendering -------------------------------------------

    pub(super) fn render_conversation(&self) {
        let buffer = self.response.buffer();
        buffer.set_text("");
        let conversation = self.conversation.borrow();
        if conversation.is_empty() {
            self.insert_tagged("Ask anything to begin.\n", "dim");
            return;
        }
        for turn in &conversation.turns {
            self.insert_tagged(&format!("{}\n", turn.role.label()), "role");
            if turn.had_image {
                self.insert_tagged("[screenshot attached] ", "dim");
            }
            let mut iter = buffer.end_iter();
            buffer.insert(&mut iter, &format!("{}\n\n", turn.text));
        }
        self.scroll_to_end();
    }

    /// Print the "Nexora" label so streamed deltas append under it.
    fn begin_assistant_line(&self) {
        self.insert_tagged(&format!("{}\n", Role::Assistant.label()), "role");
        self.scroll_to_end();
    }

    fn append_text(&self, text: &str) {
        let buffer = self.response.buffer();
        buffer.insert(&mut buffer.end_iter(), text);
        self.scroll_to_end();
    }

    /// A dim, non-conversation line (errors, hints).
    pub(super) fn show_system_line(&self, text: &str) {
        self.insert_tagged(&format!("{text}\n"), "dim");
        self.scroll_to_end();
    }

    pub(super) fn show_live_line(&self, label: &str, text: &str, tag: &str) {
        if self.stack.visible_child_name().as_deref() != Some("live") {
            self.live_button.set_label("Live •");
        }
        let buffer = self.live_response.buffer();
        let mut iter = buffer.end_iter();
        buffer.insert_with_tags_by_name(&mut iter, &format!("\n{label}\n"), &[tag]);
        buffer.insert(&mut buffer.end_iter(), &format!("{text}\n"));
        buffer.move_mark(&self.live_end_mark, &buffer.end_iter());
        self.live_response.scroll_mark_onscreen(&self.live_end_mark);
    }

    fn insert_tagged(&self, text: &str, tag: &str) {
        let buffer = self.response.buffer();
        let mut iter = buffer.end_iter();
        buffer.insert_with_tags_by_name(&mut iter, text, &[tag]);
    }

    fn scroll_to_end(&self) {
        let buffer = self.response.buffer();
        buffer.move_mark(&self.end_mark, &buffer.end_iter());
        self.response.scroll_mark_onscreen(&self.end_mark);
    }

    pub(super) fn set_status(&self, text: &str) {
        self.status.set_text(text);
        self.live_status.set_text(text);
    }
}
