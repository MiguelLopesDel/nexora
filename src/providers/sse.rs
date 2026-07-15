//! Minimal incremental Server-Sent Events parser.
//!
//! Both the Anthropic and OpenAI streaming APIs speak SSE; a full client
//! library would be overkill for the subset they use.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buffer: String,
    event: Option<String>,
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes; returns every event completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(SseEvent {
                        event: self.event.take(),
                        data: self.data_lines.join("\n"),
                    });
                    self.data_lines.clear();
                } else {
                    self.event = None;
                }
            } else if let Some(value) = field(line, "event") {
                self.event = Some(value.to_string());
            } else if let Some(value) = field(line, "data") {
                self.data_lines.push(value.to_string());
            }
            // Comments (":...") and unknown fields are ignored per spec.
        }
        events
    }
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?;
    let rest = rest.strip_prefix(':')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_style_events() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[0].event, None);
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn parses_anthropic_style_events() {
        let mut parser = SseParser::new();
        let events =
            parser.push(b"event: content_block_delta\ndata: {\"delta\":{\"text\":\"hi\"}}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
    }

    #[test]
    fn handles_events_split_across_chunks() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: hel").is_empty());
        assert!(parser.push(b"lo\n").is_empty());
        let events = parser.push(b"\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn handles_crlf_and_comments() {
        let mut parser = SseParser::new();
        let events = parser.push(b": keepalive\r\ndata: x\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn joins_multiline_data() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: a\ndata: b\n\n");
        assert_eq!(events[0].data, "a\nb");
    }
}
