// SPDX-License-Identifier: GPL-3.0-or-later
//! The installer's and the uninstaller's windows, and the work behind them.
//!
//! `G7Link-<version>-x64-setup.exe` (NSIS, `desktop/installer`) unpacks
//! the app to a temporary folder and runs it there, renamed to
//! `G7Link Setup.exe`, with `--setup --installer <setup.exe>`. That
//! window asks nothing; Install runs the same setup.exe again with
//! administrator rights and `/S /PROGRESS=<file>`, which installs into
//! Program Files and writes each step to the file, so the window can follow.
//!
//! Removing (Windows Settings > Apps) runs the installed app with `--remove`.
//! A program can't delete its own folder, so it first copies itself to a
//! temporary folder (as `G7Link Uninstall.exe`) and carries on from
//! there, then runs `uninstall.exe /S /PROGRESS=<file>` with administrator
//! rights, and finally removes this user's G7 Link data. Files received
//! from phones (`Downloads\G7Link`), voice recordings and synced folders
//! belong to the user and are kept.
//!
//! The renamed copies matter: the installer closes running copies of
//! `g7link-desktop.exe` by name, which would otherwise include these.

use crate::i18n::tr;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const SETUP_ARG: &str = "--setup";
pub const REMOVE_ARG: &str = "--remove";
pub const INSTALLER_ARG: &str = "--installer";
pub const INSTALLED_ARG: &str = "--installed";

/// Where the Android app is downloaded (the done screen's QR code and link).
pub const ANDROID_URL: &str = "https://github.com/Astraque-Softwares-Ltd/g7link/releases/latest";

const APP_EXE: &str = "g7link-desktop";

/// What the window is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Installing from `installer` (the setup .exe the user ran).
    Install { installer: PathBuf },
    /// Removing the copy in `installed`.
    Remove { installed: PathBuf },
}

/// The copy already on this PC, from its Apps entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub dir: PathBuf,
    pub version: String,
}

pub fn installed() -> Option<Installed> {
    if let Some(version) = dpkg_installed_version("g7link") {
        let dir = PathBuf::from("/usr/bin");
        if dir.join(APP_EXE).is_file() {
            return Some(Installed { dir, version });
        }
    }
    if let Some(local_data) = dirs::data_local_dir() {
        let user_dir = local_data.join("g7link-app");
        let exe = user_dir.join(APP_EXE);
        if exe.is_file() {
            let version = read_installed_version(&user_dir, &exe).unwrap_or_default();
            return Some(Installed { dir: user_dir, version });
        }
    }
    for candidate in ["/opt/g7link", "/usr/local/bin", "/usr/bin"] {
        let dir = PathBuf::from(candidate);
        let exe = dir.join(APP_EXE);
        if exe.is_file() {
            let version = read_installed_version(&dir, &exe).unwrap_or_default();
            return Some(Installed { dir, version });
        }
    }
    None
}

fn dpkg_installed_version(pkg: &str) -> Option<String> {
    let out = std::process::Command::new("dpkg-query")
        .args(["-W", "-f=${db:Status-Status}\\t${Version}\\n", pkg])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_dpkg_query_output(&String::from_utf8_lossy(&out.stdout))
}

fn parse_dpkg_query_output(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    let (status, version) = line.split_once('\t')?;
    if status.eq_ignore_ascii_case("installed") && !version.trim().is_empty() {
        Some(version.trim().to_owned())
    } else {
        None
    }
}

fn read_installed_version(dir: &Path, exe: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(dir.join("VERSION")) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let out = std::process::Command::new(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let token = text.split_whitespace().last()?.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// How this install relates to the copy already on the PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Fresh,
    Update,
    Reinstall,
    /// The PC has a newer version than this installer.
    Older,
}

pub fn kind(installed: Option<&str>, this: &str) -> Kind {
    match installed {
        None => Kind::Fresh,
        Some(v) if crate::updater::is_newer(this, v) => Kind::Update,
        Some(v) if crate::updater::is_newer(v, this) => Kind::Older,
        Some(_) => Kind::Reinstall,
    }
}

/// One line of the installer's progress file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Step(Step),
    Done,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Closing,
    Files,
    Firewall,
    Updates,
    Shortcuts,
    Cleanup,
}

impl Step {
    /// What the window says while this step runs.
    pub fn label(self) -> &'static str {
        match self {
            Step::Closing => "Closing G7 Link",
            Step::Files => "Copying files",
            Step::Firewall => "Configure Linux firewall rules",
            Step::Updates => "Setting up updates",
            Step::Shortcuts => "Adding shortcuts",
            Step::Cleanup => "Removing files",
        }
    }

    /// How far along the bar is when this step starts (copying files takes
    /// most of the time).
    pub fn progress(self) -> f64 {
        match self {
            Step::Closing => 0.04,
            Step::Files | Step::Cleanup => 0.10,
            Step::Firewall => 0.78,
            Step::Updates => 0.86,
            Step::Shortcuts => 0.94,
        }
    }
}

pub fn parse_event(line: &str) -> Option<Event> {
    let line = line.trim();
    if line == "done" {
        return Some(Event::Done);
    }
    if let Some(reason) = line.strip_prefix("error ") {
        return Some(Event::Failed(reason.trim().to_owned()));
    }
    let step = match line.strip_prefix("step ")?.trim() {
        "closing" => Step::Closing,
        "files" => Step::Files,
        "firewall" => Step::Firewall,
        "updates" => Step::Updates,
        "shortcuts" => Step::Shortcuts,
        "cleanup" => Step::Cleanup,
        _ => return None,
    };
    Some(Event::Step(step))
}

/// Where setup puts the app, when the user picks a folder (Windows, #102).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub dir: PathBuf,
    /// Only administrators can change any folder on the way to `dir`, so
    /// updates can run without asking.
    pub admin_only: bool,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// The user said no to the administrator prompt.
    Declined,
    Failed(String),
}

pub fn has_desktop_shortcut() -> bool {
    false
}

/// The installer's switches. NSIS takes `/D=` last and unquoted, spaces and all.
#[cfg(test)]
fn install_params(desktop_shortcut: bool, placement: Option<&Placement>, progress: &Path) -> String {
    let mut params = format!("/S /DESKTOP={} /PROGRESS={}", u8::from(desktop_shortcut), progress.display());
    if let Some(placement) = placement {
        params.push_str(&format!(
            " /ADMINONLY={} /D={}",
            u8::from(placement.admin_only),
            placement.dir.display()
        ));
    }
    params
}

pub fn install(
    installer: &Path,
    _desktop_shortcut: bool,
    _placement: Option<&Placement>,
    mut on_event: impl FnMut(Event),
) -> Outcome {
    let name = installer.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if name.ends_with(".deb") {
        on_event(Event::Step(Step::Closing));
        on_event(Event::Step(Step::Files));
        let path_str = installer.to_string_lossy();
        match crate::win::elevated::run_elevated(
            Path::new("sh"),
            &["-e", "-c", "apt-get install -y -- \"$1\" || dpkg -i -- \"$1\"", "sh", &path_str],
        ) {
            Ok(0) => {
                on_event(Event::Step(Step::Shortcuts));
                on_event(Event::Done);
                Outcome::Done
            }
            Ok(126 | 127) => Outcome::Declined,
            Ok(code) => Outcome::Failed(tr!("Setup", "Setup stopped with code %1.", code)),
            Err(e) => Outcome::Failed(tr!("Setup", "Couldn't start setup: %1", e)),
        }
    } else if name.ends_with(".tar.gz") {
        on_event(Event::Step(Step::Closing));
        on_event(Event::Step(Step::Files));
        let path_str = installer.to_string_lossy();
        match crate::win::elevated::run_elevated(
            Path::new("sh"),
            &[
                "-e",
                "-c",
                "dir=$(mktemp -d) && trap 'rm -rf \"$dir\"' EXIT && tar -C \"$dir\" -xzf \"$1\" && set -- \"$dir\"/G7Link-*/install.sh && [ -x \"$1\" ] && \"$1\"",
                "sh",
                &path_str,
            ],
        ) {
            Ok(0) => {
                on_event(Event::Step(Step::Shortcuts));
                on_event(Event::Done);
                Outcome::Done
            }
            Ok(126 | 127) => Outcome::Declined,
            Ok(code) => Outcome::Failed(tr!("Setup", "Setup stopped with code %1.", code)),
            Err(e) => Outcome::Failed(tr!("Setup", "Couldn't start setup: %1", e)),
        }
    } else {
        // Only the .deb and the tarball install this way; the Windows
        // installer's switches mean nothing to anything else.
        Outcome::Failed(tr!("Setup", "Couldn't start setup: %1", name))
    }
}

pub fn remove(installed: &Path, mut on_event: impl FnMut(Event)) -> Outcome {
    on_event(Event::Step(Step::Closing));
    let _ = crate::win::autostart::set_enabled(false);
    crate::win::jumplist::unregister();
    let _ = crate::send_to::remove_all();
    crate::storage::unregister_all();

    on_event(Event::Step(Step::Cleanup));
    if let Some(local_data) = dirs::data_local_dir() {
        let user_app_dir = local_data.join("g7link-app");
        if installed == user_app_dir {
            let _ = std::fs::remove_dir_all(&user_app_dir);
            if let Some(home) = dirs::home_dir() {
                let _ = std::fs::remove_file(home.join(".local/bin/g7link-desktop"));
            }
            let _ = std::fs::remove_file(local_data.join("applications/g7link.desktop"));
            let _ = std::fs::remove_file(local_data.join("metainfo/org.g7link.desktop.metainfo.xml"));
            remove_user_data();
            on_event(Event::Done);
            return Outcome::Done;
        }
    }
    let res = if dpkg_installed_version("g7link").is_some()
        || Path::new("/var/lib/dpkg/info/g7link.list").is_file()
    {
        crate::win::elevated::run_elevated(Path::new("dpkg"), &["-r", "g7link"])
    } else if installed == Path::new("/opt/g7link") {
        crate::win::elevated::run_elevated(
            Path::new("rm"),
            &["-rf", "--", "/opt/g7link", "/usr/local/bin/g7link-desktop"],
        )
    } else {
        crate::win::elevated::run_elevated(
            Path::new("rm"),
            &["-rf", "--", "/opt/g7link", "/usr/local/bin/g7link-desktop", "/usr/bin/g7link-desktop"],
        )
    };
    match res {
        Ok(0) => {
            remove_user_data();
            on_event(Event::Done);
            Outcome::Done
        }
        Ok(126 | 127) => Outcome::Declined,
        Ok(code) => Outcome::Failed(tr!("Setup", "Uninstall stopped with code %1.", code)),
        Err(e) => Outcome::Failed(tr!("Setup", "Couldn't uninstall G7 Link: %1", e)),
    }
}

pub fn remove_user_data() {
    let Some(data) = user_data_dir() else { return };
    if let Err(e) = std::fs::remove_dir_all(&data)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, "can't remove all of G7 Link's data");
    }
}

fn user_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("G7Link"))
}

/// Runs `program` elevated with `params` and follows `progress` until it
/// reports done or an error. With `wait_for_exit`, the process ending also
/// ends the run (its exit code decides when the file says nothing).
#[allow(dead_code)]
fn run_tracked(
    program: &Path,
    params: &str,
    progress: &Path,
    limit: Duration,
    wait_for_exit: bool,
    mut on_event: impl FnMut(Event),
) -> Outcome {
    let _ = std::fs::remove_file(progress);
    let process = match crate::win::elevated::start(program, params) {
        Ok(process) => process,
        Err(crate::win::elevated::Error::Declined) => return Outcome::Declined,
        Err(crate::win::elevated::Error::Other(e)) => {
            return Outcome::Failed(tr!("Setup", "Couldn't start setup: %1", e));
        }
    };
    let started = Instant::now();
    let mut read = 0usize;
    loop {
        for event in new_events(progress, &mut read) {
            match event {
                Event::Done => {
                    let _ = std::fs::remove_file(progress);
                    return Outcome::Done;
                }
                Event::Failed(reason) => {
                    let _ = std::fs::remove_file(progress);
                    return Outcome::Failed(reason);
                }
                step @ Event::Step(_) => on_event(step),
            }
        }
        if wait_for_exit && let Some(code) = process.exit_code() {
            // Read what it wrote just before exiting.
            for event in new_events(progress, &mut read) {
                match event {
                    Event::Done => return Outcome::Done,
                    Event::Failed(reason) => return Outcome::Failed(reason),
                    step @ Event::Step(_) => on_event(step),
                }
            }
            let _ = std::fs::remove_file(progress);
            return if code == 0 {
                Outcome::Done
            } else {
                Outcome::Failed(tr!("Setup", "Setup stopped with code %1.", code))
            };
        }
        if started.elapsed() > limit {
            return Outcome::Failed(tr!(
                "Setup",
                "Setup is taking too long. Try again after restarting the PC."
            ));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// Lines added to the progress file since the last call.
fn new_events(path: &Path, read: &mut usize) -> Vec<Event> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    // A line still being written has no newline yet: only whole lines count.
    let whole = text.rfind('\n').map_or("", |end| &text[..=end]);
    let lines: Vec<&str> = whole.lines().collect();
    let fresh: Vec<Event> = lines.iter().skip(*read).filter_map(|l| parse_event(l)).collect();
    *read = lines.len();
    fresh
}

#[allow(dead_code)]
fn progress_file(what: &str) -> PathBuf {
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
        let dir = dirs::runtime_dir()
            .or_else(dirs::cache_dir)
            .filter(|p| p.is_absolute())
            .map(|base| base.join("g7link"))
            .unwrap_or_else(|| std::env::temp_dir().join(format!("g7link-{uid}")));
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
        dir.join(format!("g7link-{what}-{}.progress", std::process::id()))
    }
}

pub fn open_app(dir: &Path) -> Result<(), String> {
    let app = dir.join(APP_EXE);
    if !app.is_file() {
        return Err(format!("{} is missing", app.display()));
    }
    std::process::Command::new(&app)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn hand_over_removal(_installed: &Path) -> Result<bool, String> {
    // On Unix, open executables can be unlinked while running, and `installed`
    // may be `/usr/bin` which must never be copied into `/tmp`.
    Ok(false)
}

pub fn delete_own_copy_later() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let temp = std::env::temp_dir();
    let ours = dir.starts_with(&temp)
        && dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("G7Link-uninstall-"));
    if !ours {
        return;
    }
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 3 && rm -rf \"$1\"")
        .arg("sh")
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The window's QR code for the Android app, as an SVG data URL.
pub fn android_qr(dark: &str, light: &str) -> String {
    match crate::qr::encode(ANDROID_URL) {
        Ok(qr) => format!(
            "data:image/svg+xml;utf8,<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-2 -2 {s} {s}\" \
             shape-rendering=\"crispEdges\"><rect x=\"-2\" y=\"-2\" width=\"{s}\" height=\"{s}\" fill=\"{light}\"/>\
             <path d=\"{path}\" fill=\"{dark}\"/></svg>",
            s = qr.size + 4,
            path = qr.path,
            light = light.replace('#', "%23"),
            dark = dark.replace('#', "%23"),
        ),
        Err(_) => String::new(),
    }
}

/// The app icon as a PNG data URL, at `size` pixels.
pub fn icon_url(size: usize) -> String {
    use data_encoding::BASE64;
    let mut bgra = crate::mark::render(size);
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let bitmap = crate::win::image::Bitmap { width: size as u32, height: size as u32, bgra };
    crate::win::image::encode_png(&bitmap)
        .map(|png| format!("data:image/png;base64,{}", BASE64.encode(&png)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_folder_goes_last_for_nsis() {
        let progress = Path::new("p.txt");
        assert_eq!(install_params(true, None, progress), "/S /DESKTOP=1 /PROGRESS=p.txt");
        let placement = Placement { dir: PathBuf::from(r"D:\My Apps\G7Link"), admin_only: false };
        assert_eq!(
            install_params(false, Some(&placement), progress),
            r"/S /DESKTOP=0 /PROGRESS=p.txt /ADMINONLY=0 /D=D:\My Apps\G7Link"
        );
    }

    #[test]
    fn progress_lines_parse() {
        assert_eq!(parse_event("step files"), Some(Event::Step(Step::Files)));
        assert_eq!(parse_event("step firewall\r"), Some(Event::Step(Step::Firewall)));
        assert_eq!(parse_event("done"), Some(Event::Done));
        assert_eq!(parse_event("error Not enough space"), Some(Event::Failed("Not enough space".into())));
        assert_eq!(parse_event("step something-new"), None);
        assert_eq!(parse_event(""), None);
    }

    #[test]
    fn steps_only_move_the_bar_forward() {
        let order = [Step::Closing, Step::Files, Step::Firewall, Step::Updates, Step::Shortcuts];
        assert!(order.windows(2).all(|w| w[0].progress() < w[1].progress()));
    }

    #[test]
    fn install_kind_compares_versions() {
        assert_eq!(kind(None, "0.1.0"), Kind::Fresh);
        assert_eq!(kind(Some("0.1.0"), "0.1.1"), Kind::Update);
        assert_eq!(kind(Some("0.1.0"), "0.1.0"), Kind::Reinstall);
        assert_eq!(kind(Some("0.2.0"), "0.1.0"), Kind::Older);
    }

    #[test]
    fn dpkg_query_output_parses_only_installed_packages() {
        assert_eq!(parse_dpkg_query_output("installed\t0.1.0\n"), Some("0.1.0".to_owned()));
        assert_eq!(parse_dpkg_query_output("config-files\t0.1.0\n"), None);
        assert_eq!(parse_dpkg_query_output("not-installed\t\n"), None);
    }

    #[test]
    fn new_events_reads_only_what_was_added() {
        let dir = std::env::temp_dir().join(format!("g7link-setup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.progress");
        std::fs::write(&file, "step closing\nstep files\n").unwrap();
        let mut read = 0;
        assert_eq!(new_events(&file, &mut read), vec![Event::Step(Step::Closing), Event::Step(Step::Files)]);
        assert_eq!(new_events(&file, &mut read), vec![]);
        std::fs::write(&file, "step closing\nstep files\nstep firewall\ndone\n").unwrap();
        assert_eq!(new_events(&file, &mut read), vec![Event::Step(Step::Firewall), Event::Done]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_tree_copies_nested_files() {
        let base = std::env::temp_dir().join(format!("g7link-copytree-{}", std::process::id()));
        let from = base.join("from");
        std::fs::create_dir_all(from.join("qml/QtQuick")).unwrap();
        std::fs::write(from.join("a.dll"), b"a").unwrap();
        std::fs::write(from.join("qml/QtQuick/qmldir"), b"q").unwrap();
        copy_tree(&from, &base.join("to")).unwrap();
        assert_eq!(std::fs::read(base.join("to/qml/QtQuick/qmldir")).unwrap(), b"q");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn qr_is_an_svg_data_url() {
        let url = android_qr("#2C1600", "#FFFFFF");
        assert!(url.starts_with("data:image/svg+xml;utf8,<svg"));
        assert!(url.contains("%232C1600"));
    }
}
