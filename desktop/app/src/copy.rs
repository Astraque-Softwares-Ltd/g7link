// SPDX-License-Identifier: GPL-3.0-or-later
//! Copying to the PC clipboard without holding up the window. Putting a photo
//! on the clipboard reads the file, decodes it and encodes a PNG, which takes
//! seconds for a big one, so that runs on a blocking thread and the app says
//! how it went when it's done. Every "copy image" button goes through here.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{bridge::app::show_message, core_host, i18n::tr};

/// Files bigger than this get a "Copying…" first; smaller ones are done
/// before it would have shown.
const SAY_COPYING_ABOVE: u64 = 1024 * 1024;

/// The clipboard type for an image file, by its name.
pub(crate) fn image_mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        _ => "image/jpeg",
    }
}

/// What to say about an image copy.
pub(crate) struct ImageCopy {
    /// Shown when it worked; `None` says nothing (and no "Copying…" either).
    copied: Option<String>,
    /// Shown when the file can't be read or put on the clipboard.
    failed: String,
    /// Shown when the file is gone (else `failed`).
    missing: Option<String>,
}

impl ImageCopy {
    /// Says "Copied" when it worked and `failed` when it didn't.
    pub(crate) fn new(failed: String) -> Self {
        ImageCopy { copied: Some(tr!("Copy", "Copied")), failed, missing: None }
    }

    /// Says `missing` when the file is gone.
    pub(crate) fn when_missing(self, missing: String) -> Self {
        ImageCopy { missing: Some(missing), ..self }
    }

    /// Says only when it didn't work.
    pub(crate) fn quiet(self) -> Self {
        ImageCopy { copied: None, ..self }
    }

    fn message(&self, outcome: &Result<(), CopyError>) -> Option<&str> {
        match outcome {
            Ok(()) => self.copied.as_deref(),
            Err(CopyError::Missing) => Some(self.missing.as_deref().unwrap_or(&self.failed)),
            Err(CopyError::Failed(_)) => Some(&self.failed),
        }
    }
}

/// Why an image didn't get copied.
#[derive(Debug, PartialEq)]
enum CopyError {
    Missing,
    Failed(String),
}

/// Puts the image at `path` on the clipboard off the window's thread, then
/// says how it went and runs `after` with whether it worked.
pub(crate) fn copy_image_file(path: PathBuf, say: ImageCopy, after: impl FnOnce(bool) + Send + 'static) {
    if say.copied.is_some() && std::fs::metadata(&path).is_ok_and(|m| m.len() > SAY_COPYING_ABOVE) {
        show_message(tr!("Copy", "Copying…"));
    }
    core_host::spawn(async move {
        let outcome = tokio::task::spawn_blocking(move || {
            let outcome = copy_image(&path, crate::sys::clipboard::write_image);
            after(outcome.is_ok());
            outcome
        })
        .await
        .unwrap_or_else(|e| Err(CopyError::Failed(e.to_string())));
        if let Err(CopyError::Failed(reason)) = &outcome {
            tracing::warn!(reason, "can't copy a photo");
        }
        if let Some(message) = say.message(&outcome) {
            show_message(message);
        }
    });
}

fn copy_image(path: &Path, write: impl FnOnce(&str, &[u8]) -> Result<(), String>) -> Result<(), CopyError> {
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => CopyError::Missing,
        _ => CopyError::Failed(e.to_string()),
    })?;
    write(image_mime(path), &bytes).map_err(CopyError::Failed)
}

/// Puts `text` on the clipboard off the window's thread (it can start
/// `wl-copy`), and says so only when it didn't work: the caller says
/// "Copied" right away.
pub(crate) fn copy_text(text: String) {
    core_host::spawn(async move {
        let written = tokio::task::spawn_blocking(move || crate::sys::clipboard::write(&text))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(reason) = written {
            tracing::warn!(reason, "can't copy");
            show_message(tr!("Copy", "Couldn't copy"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say() -> ImageCopy {
        ImageCopy { copied: Some("copied".into()), failed: "failed".into(), missing: None }
    }

    #[test]
    fn the_mime_comes_from_the_extension() {
        assert_eq!(image_mime(Path::new("a/b.png")), "image/png");
        assert_eq!(image_mime(Path::new("a/b.PNG")), "image/png");
        assert_eq!(image_mime(Path::new("a/b.jpg")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a/b.jpeg")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a/b.heic")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a/b")), "image/jpeg");
    }

    #[test]
    fn a_missing_file_says_it_was_moved_or_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut wrote = false;
        let outcome = copy_image(&dir.path().join("gone.jpg"), |_, _| {
            wrote = true;
            Ok(())
        });
        assert_eq!(outcome, Err(CopyError::Missing));
        assert!(!wrote);
        let named = say().when_missing("moved or deleted".into());
        assert_eq!(named.message(&outcome), Some("moved or deleted"));
        assert_eq!(say().message(&outcome), Some("failed"));
    }

    #[test]
    fn the_file_goes_to_the_clipboard_with_its_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.PNG");
        std::fs::write(&path, b"pixels").unwrap();
        let mut seen = None;
        let outcome = copy_image(&path, |mime, bytes| {
            seen = Some((mime.to_owned(), bytes.to_vec()));
            Ok(())
        });
        assert_eq!(outcome, Ok(()));
        assert_eq!(seen, Some(("image/png".to_owned(), b"pixels".to_vec())));
        assert_eq!(say().message(&outcome), Some("copied"));
        assert_eq!(say().quiet().message(&outcome), None);
    }

    #[test]
    fn a_clipboard_failure_says_so_even_when_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        std::fs::write(&path, b"not an image").unwrap();
        let outcome = copy_image(&path, |_, _| Err("bad image".into()));
        assert_eq!(outcome, Err(CopyError::Failed("bad image".into())));
        assert_eq!(say().quiet().message(&outcome), Some("failed"));
    }
}
