//! Screen capture through the XDG desktop portal.
//!
//! `org.freedesktop.portal.Screenshot` is the one capture API that works
//! across GNOME, KDE, wlroots compositors, COSMIC and X11 sessions.

use anyhow::{Context, Result};
use ashpd::desktop::screenshot::Screenshot;

/// Capture the screen and return the PNG bytes.
///
/// The portal may show a permission dialog on first use (GNOME/KDE remember
/// the choice). The temporary file the portal writes is removed after read.
pub async fn capture_png() -> Result<Vec<u8>> {
    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
        .context("screenshot portal request failed — is xdg-desktop-portal running?")?
        .response()
        .context("screenshot was denied or cancelled")?;

    let uri = response.uri().as_str().to_string();
    let path =
        file_uri_to_path(&uri).with_context(|| format!("portal returned a non-file URI: {uri}"))?;
    let bytes = std::fs::read(&path).with_context(|| format!("reading screenshot at {path}"))?;
    let _ = std::fs::remove_file(&path);
    Ok(bytes)
}

/// Convert a file:// URI into a filesystem path, percent-decoding it.
fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    // Drop an authority component (usually empty: file:///path).
    let path = &rest[rest.find('/')?..];
    percent_decode(path)
}

fn percent_decode(text: &str) -> Option<String> {
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_file_uri() {
        assert_eq!(
            file_uri_to_path("file:///tmp/shot.png").as_deref(),
            Some("/tmp/shot.png")
        );
    }

    #[test]
    fn percent_encoded_uri() {
        assert_eq!(
            file_uri_to_path("file:///home/u/Screenshot%20from%202026.png").as_deref(),
            Some("/home/u/Screenshot from 2026.png")
        );
    }

    #[test]
    fn non_file_uri_is_rejected() {
        assert_eq!(file_uri_to_path("https://example.com/x.png"), None);
    }

    #[test]
    fn truncated_percent_escape_is_rejected() {
        assert_eq!(percent_decode("bad%2"), None);
    }
}
