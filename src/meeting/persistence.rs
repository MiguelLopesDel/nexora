//! Saving a finished session to a private markdown file, and the WAV framing
//! used to upload chunks to remote transcription APIs.

use std::path::PathBuf;

use anyhow::Result;

pub(super) const SAMPLE_RATE: u32 = 16_000;
pub(super) const CHANNELS: u16 = 1;
pub(super) const BITS_PER_SAMPLE: u16 = 16;

pub(super) fn pcm_to_wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * BITS_PER_SAMPLE as u32 / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

pub(super) fn save_session(
    transcript: &[String],
    translations: &[String],
    notes: &[String],
    summary: Option<&str>,
) -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("nexora")
        .join("sessions");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("session-{id}.md"));
    let mut document = format!(
        "# Nexora Session {id}\n\n## Transcript\n\n{}\n",
        transcript.join("\n\n")
    );
    if !translations.is_empty() {
        document.push_str(&format!(
            "\n## Translation\n\n{}\n",
            translations.join("\n\n")
        ));
    }
    if !notes.is_empty() {
        document.push_str(&format!(
            "\n## Live Coaching and Notes\n\n{}\n",
            notes.join("\n\n")
        ));
    }
    if let Some(summary) = summary {
        document.push_str(&format!("\n## Summary\n\n{summary}\n"));
    }
    write_private(&path, document.as_bytes())?;
    Ok(path)
}

fn write_private(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents)?;
    Ok(())
}

pub(super) fn excerpt(text: &str) -> String {
    text.chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_contains_pcm_size() {
        let wav = pcm_to_wav(&[0; 320]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 320);
        assert_eq!(wav.len(), 364);
    }
}
