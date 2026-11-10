// SPDX-License-Identifier: GPL-3.0-or-later
// What the desktop entry passes to share: files, `file://` URIs, links
// (`g7link://`, `tel:`, web pages) or text.

use std::path::PathBuf;

/// Command-line flag to share files, URLs, or text with a paired phone.
pub const ARG: &str = "--share";

/// What to share, from the desktop entry's arguments or `--share`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedPayload {
    pub files: Vec<PathBuf>,
    pub url: Option<String>,
    pub text: Option<String>,
}

impl SharedPayload {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.url.is_none() && self.text.is_none()
    }
}

/// Parse command-line targets (`%U` / `%F` from `.desktop` Exec lines, `file://` URIs,
/// `http(s)://` / `g7link://` URLs, local paths, or plain text) into a [`SharedPayload`].
pub fn parse_targets(targets: &[String]) -> SharedPayload {
    let mut payload = SharedPayload::default();
    let mut text_parts = Vec::new();

    for raw in targets {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(file_path) = parse_file_uri(trimmed) {
            payload.files.push(file_path);
        } else if is_url_target(trimmed) {
            if payload.url.is_none() {
                payload.url = Some(trimmed.to_string());
            } else {
                text_parts.push(trimmed.to_string());
            }
        } else {
            let expanded = expand_tilde(trimmed);
            if expanded.exists() || looks_like_local_path(trimmed) {
                payload.files.push(expanded);
            } else {
                text_parts.push(trimmed.to_string());
            }
        }
    }

    if !text_parts.is_empty() {
        payload.text = Some(text_parts.join(" "));
    }

    payload
}

fn parse_file_uri(s: &str) -> Option<PathBuf> {
    let rest = s.strip_prefix("file://")?;
    let path_part = if let Some(after_localhost) = rest.strip_prefix("localhost/") {
        format!("/{after_localhost}")
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    };
    Some(PathBuf::from(percent_decode(&path_part)))
}

fn is_url_target(s: &str) -> bool {
    if s.is_empty() || s.len() > 4096 || s.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with("g7link:")
        || (lower.starts_with("geo:") && s.len() > 4)
}

fn looks_like_local_path(s: &str) -> bool {
    let bytes = s.as_bytes();
    s.contains('/')
        || s.contains('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(s)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_targets_handles_file_uris_with_percent_encoding() {
        let targets = vec![
            "file:///home/alice/My%20Documents/report%20final.pdf".to_string(),
            "file://localhost/tmp/photo%2B1.png".to_string(),
        ];
        let payload = parse_targets(&targets);
        assert_eq!(
            payload.files,
            vec![
                PathBuf::from("/home/alice/My Documents/report final.pdf"),
                PathBuf::from("/tmp/photo+1.png"),
            ]
        );
        assert!(payload.url.is_none());
        assert!(payload.text.is_none());
    }

    #[test]
    fn parse_targets_handles_urls_and_plain_text() {
        let targets =
            vec!["https://example.com/article?id=42".to_string(), "Check out this article".to_string()];
        let payload = parse_targets(&targets);
        assert!(payload.files.is_empty());
        assert_eq!(payload.url.as_deref(), Some("https://example.com/article?id=42"));
        assert_eq!(payload.text.as_deref(), Some("Check out this article"));
    }

    #[test]
    fn parse_targets_handles_g7link_scheme_and_local_paths() {
        let targets =
            vec!["g7link://share?text=hello".to_string(), "/var/tmp/some-archive.tar.gz".to_string()];
        let payload = parse_targets(&targets);
        assert_eq!(payload.url.as_deref(), Some("g7link://share?text=hello"));
        assert_eq!(payload.files, vec![PathBuf::from("/var/tmp/some-archive.tar.gz")]);
    }

    #[test]
    fn parse_targets_returns_empty_on_empty_or_whitespace() {
        assert!(parse_targets(&[]).is_empty());
        assert!(parse_targets(&["   ".to_string()]).is_empty());
    }
}
