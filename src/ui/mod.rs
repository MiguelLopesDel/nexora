mod overlay;

pub use overlay::Overlay;
pub use overlay::append_meeting_transcript_context;

pub const STYLE: &str = include_str!("style.css");
