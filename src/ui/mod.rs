mod conversation_view;
mod overlay;
mod session;
mod settings;
mod window;

pub use overlay::Overlay;
pub use session::append_meeting_transcript_context;

pub const STYLE: &str = include_str!("style.css");
