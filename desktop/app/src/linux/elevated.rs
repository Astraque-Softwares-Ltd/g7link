// SPDX-License-Identifier: GPL-3.0-or-later
//! Running a process with root privileges on Linux via `pkexec` (Polkit).

use std::{fs, path::Path, process::Command};

/// Why starting or running the elevated process failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The user dismissed or declined the Polkit prompt (`pkexec` exit status 126 or 127).
    Declined,
    Other(String),
}

impl Error {
    #[allow(non_snake_case)]
    pub fn Failed(msg: String) -> Self {
        Self::Other(msg)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declined => write!(f, "authentication declined"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

unsafe extern "C" {
    fn geteuid() -> u32;
}

/// Returns `true` if the current process is already running as `root` (`euid == 0`).
pub fn is_elevated() -> bool {
    // SAFETY: `geteuid` is always safe and has no preconditions.
    unsafe { geteuid() == 0 }
}

fn has_pkexec() -> bool {
    use std::os::unix::fs::MetadataExt;

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let extra = [Path::new("/usr/bin"), Path::new("/bin")];
    std::env::split_paths(&path_var).chain(extra.into_iter().map(Path::to_path_buf)).any(|dir| {
        let candidate = dir.join("pkexec");
        fs::metadata(&candidate).is_ok_and(|m| {
            if !m.is_file() {
                return false;
            }
            m.mode() & 0o111 != 0
        })
    })
}

pub fn classify_exit_code(code: Option<i32>, ran_via_pkexec: bool) -> Result<i32, Error> {
    match code {
        Some(126 | 127) if ran_via_pkexec => Err(Error::Declined),
        Some(c) => Ok(c),
        None => Ok(1),
    }
}

/// Runs `program` with `args` under `pkexec` (or directly when already `root`)
/// and waits for its exit code, returning `Err(Error::Declined)` when `pkexec`
/// exits with `126` or `127`.
pub fn run(program: &Path, args: &[&str]) -> Result<i32, Error> {
    let elevated = is_elevated();
    if !elevated && !has_pkexec() {
        return Err(Error::Failed("pkexec not found".into()));
    }

    let mut cmd = if elevated {
        let mut c = Command::new(program);
        c.args(args);
        c
    } else {
        let mut c = Command::new("pkexec");
        c.arg(program).args(args);
        c
    };
    let status = cmd.status().map_err(|e| Error::Failed(e.to_string()))?;
    classify_exit_code(status.code(), !elevated)
}

/// Runs `program` with `args` under `pkexec` and waits for its exit code.
pub fn run_elevated(program: &Path, args: &[&str]) -> Result<i32, String> {
    run(program, args).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pkexec_dismissal_and_denial_codes_as_declined() {
        assert_eq!(classify_exit_code(Some(126), true), Err(Error::Declined));
        assert_eq!(classify_exit_code(Some(127), true), Err(Error::Declined));
        assert_eq!(classify_exit_code(Some(0), true), Ok(0));
        assert_eq!(classify_exit_code(Some(1), true), Ok(1));
        assert_eq!(classify_exit_code(Some(127), false), Ok(127));
    }
}
