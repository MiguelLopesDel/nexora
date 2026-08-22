//! Prompt construction for live coaching, translation, and session summaries.

use crate::config::{MeetingConfig, TaskConfig};
use crate::conversation::Role;
use crate::providers::ChatRequest;

pub(super) fn request(
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

pub(super) fn coaching_prompt(
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

pub(super) fn recent_transcript(chunks: &[String], max_chars: usize) -> String {
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

pub(super) fn session_summary_prompt(notes: &[String], transcript: &[String]) -> String {
    format!(
        "Summarize this session using the grounding rules below.\n- The transcript is the source of truth.\n- Generated notes are untrusted model suggestions, not evidence. Include a note only when the transcript independently supports it.\n- A question is not a decision. A proposed reply is not something a participant actually said.\n- If notes conflict with or go beyond the transcript, discard them.\n- Do not infer identities, intent, domain, policies, location, costs, owners, or action items.\n\nInclude only supported decisions, key points, objections, open questions, and action items with owners when explicitly stated. Say when the available transcript is insufficient.\n\nGenerated rolling notes (untrusted):\n{}\n\nRecent transcript (source of truth):\n{}",
        recent_transcript(notes, 20_000),
        recent_transcript(transcript, 40_000),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_transcript_keeps_latest_chunks() {
        let chunks = vec!["old text".into(), "middle".into(), "latest".into()];
        assert_eq!(recent_transcript(&chunks, 13), "middle\nlatest");
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
