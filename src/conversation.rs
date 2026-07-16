//! Conversation state: multi-turn context plus on-disk history.
//!
//! Images are deliberately *not* persisted and *not* replayed on follow-up
//! turns — they are the expensive part of a request, so an attached screenshot
//! only counts for the single turn it belongs to. The text of that turn stays
//! in context; a "[screenshot]" marker records that one was attached.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn label(&self) -> &'static str {
        match self {
            Role::User => "You",
            Role::Assistant => "Nexora",
        }
    }
}

/// One persisted turn. `had_image` records that a screenshot was attached
/// without storing the pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    #[serde(default)]
    pub had_image: bool,
}

/// A conversation, identified by the timestamp it was started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub turns: Vec<Turn>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            id: new_id(),
            turns: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    pub fn push_user(&mut self, text: String, had_image: bool) {
        self.turns.push(Turn {
            role: Role::User,
            text,
            had_image,
        });
    }

    pub fn push_assistant(&mut self, text: String) {
        self.turns.push(Turn {
            role: Role::Assistant,
            text,
            had_image: false,
        });
    }

    /// Text-only (role, content) pairs for the API request context.
    pub fn api_messages(&self) -> Vec<(Role, String)> {
        self.turns
            .iter()
            .map(|turn| (turn.role, turn.text.clone()))
            .collect()
    }

    pub fn save(&self) -> Result<()> {
        if self.turns.is_empty() {
            return Ok(());
        }
        let dir = history_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.id));
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Load the most recently modified conversation, if any.
    pub fn load_latest() -> Option<Self> {
        let dir = history_dir();
        let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            if entry.path().extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
                newest = Some((modified, entry.path()));
            }
        }
        let (_, path) = newest?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

fn history_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("nexora")
        .join("history")
}

/// Filesystem- and sort-friendly id: `20260715-133742-000`.
fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Cheap UTC breakdown without pulling in a datetime crate.
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let days = secs / 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let tod = secs % 86_400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{millis:03}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
    )
}

/// Today's UTC date as `YYYY-MM-DD`, without pulling in a datetime crate.
pub fn utc_date_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(18_993), (2022, 1, 1));
        // 2026-07-15
        assert_eq!(civil_from_days(20_649), (2026, 7, 15));
    }

    #[test]
    fn api_messages_preserve_order_and_roles() {
        let mut conversation = Conversation::new();
        conversation.push_user("hi".into(), false);
        conversation.push_assistant("hello".into());
        conversation.push_user("more".into(), true);
        let messages = conversation.api_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], (Role::User, "hi".into()));
        assert_eq!(messages[1], (Role::Assistant, "hello".into()));
        assert_eq!(messages[2].0, Role::User);
    }

    #[test]
    fn roundtrips_through_json_without_images() {
        let mut conversation = Conversation::new();
        conversation.push_user("q".into(), true);
        conversation.push_assistant("a".into());
        let json = serde_json::to_string(&conversation).unwrap();
        // The flag is persisted, but never any pixel data.
        assert!(json.contains("had_image"));
        assert!(!json.contains("image_png"));
        assert!(!json.contains("base64"));
        let back: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.turns.len(), 2);
        assert!(back.turns[0].had_image);
    }
}
