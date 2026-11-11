// SPDX-License-Identifier: GPL-3.0-or-later
//! Builds the QML module `app.g7link` (QML files compiled ahead of time,
//! Rust QObjects, C++ helpers), and compiles in the bundled fonts, shaders
//! and translations.

use std::{
    fs,
    path::{Path, PathBuf},
};

use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

const QML: &[&str] = &[
    "qml/App.qml",
    "qml/IslandWindow.qml",
    "qml/LaserOverlay.qml",
    "qml/MainWindow.qml",
    "qml/MirrorWindow.qml",
    "qml/ShelfWindow.qml",
    "qml/components/ActionRow.qml",
    "qml/components/AppsSheet.qml",
    "qml/components/Avatar.qml",
    "qml/components/Button.qml",
    "qml/components/CallCard.qml",
    "qml/components/CaptionButtons.qml",
    "qml/components/Card.qml",
    "qml/components/Chip.qml",
    "qml/components/CodeDigits.qml",
    "qml/components/CommandPalette.qml",
    "qml/components/Divider.qml",
    "qml/components/GlassRim.qml",
    "qml/components/DoctorSheet.qml",
    "qml/components/Field.qml",
    "qml/components/HistoryItem.qml",
    "qml/components/Icon.qml",
    "qml/components/IconButton.qml",
    "qml/components/Keycap.qml",
    "qml/components/ListRow.qml",
    "qml/components/LockChip.qml",
    "qml/components/NotificationItem.qml",
    "qml/components/NowPlaying.qml",
    "qml/components/PopupMenu.qml",
    "qml/components/QrCode.qml",
    "qml/components/RoundedImage.qml",
    "qml/components/SectionLabel.qml",
    "qml/components/Segmented.qml",
    "qml/components/Sheet.qml",
    "qml/components/Spinner.qml",
    "qml/components/StarPrompt.qml",
    "qml/components/StatusDot.qml",
    "qml/components/Toast.qml",
    "qml/components/Toggle.qml",
    "qml/components/TransferItem.qml",
    "qml/components/ValueSlider.qml",
    "qml/components/WindowPlacement.qml",
    "qml/components/SwitchBar.qml",
    "qml/components/TextAction.qml",
    "qml/components/Txt.qml",
    "qml/pages/AppsPage.qml",
    "qml/pages/CallsPage.qml",
    "qml/pages/DeckPage.qml",
    "qml/pages/FeaturesGuidePage.qml",
    "qml/pages/HomePage.qml",
    "qml/pages/MessagesPage.qml",
    "qml/pages/PairingPanel.qml",
    "qml/pages/PhotosPage.qml",
    "qml/pages/SettingsPage.qml",
    "qml/pages/TimelinePage.qml",
    "qml/pages/WalkthroughPage.qml",
    "qml/pages/WelcomePage.qml",
    "qml/components/HiveBackdrop.qml",
];

/// QML singletons.
const QML_SINGLETONS: &[&str] = &["qml/Tokens.qml", "qml/Theme.qml", "qml/Icons.qml", "qml/G7Glyphs.qml"];

/// Bundled fonts (in `assets/fonts`), compiled in under `:/fonts/`.
const FONTS: &[&str] = &[
    "Figtree.ttf",
    "GeistMono-Variable.ttf",
    "InstrumentSerif-Regular.ttf",
    "SpaceMono-Regular.ttf",
    "SpaceMono-Bold.ttf",
];

fn write_if_changed(path: &Path, content: impl AsRef<[u8]>) {
    let bytes = content.as_ref();
    if fs::read(path).ok().as_deref() != Some(bytes) {
        fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }
}

/// Writes a resource file listing the fonts (they live outside this crate,
/// so each needs an alias) and returns its path.
fn fonts_qrc() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    let mut qrc = String::from("<RCC>\n  <qresource prefix=\"/fonts\">\n");
    for font in FONTS {
        let path = dir.join(font).canonicalize().unwrap_or_else(|e| panic!("missing font {font}: {e}"));
        println!("cargo::rerun-if-changed={}", path.display());
        // rcc takes forward slashes; strip the verbatim prefix canonicalize adds.
        let path = path.display().to_string().trim_start_matches(r"\\?\").replace('\\', "/");
        qrc.push_str(&format!("    <file alias=\"{font}\">{path}</file>\n"));
    }
    qrc.push_str("  </qresource>\n</RCC>\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("fonts.qrc");
    write_if_changed(&out, qrc);
    out
}

/// Fragment shaders, served under `qrc:/shaders/`.
const SHADERS: &[&str] = &["glassrim.frag", "hive.frag"];

/// Compiles the shaders with Qt's `qsb` when it can be found (next to qmake, or on PATH),
/// otherwise uses the compiled copies committed beside the sources. Returns the qrc path.
fn shaders_qrc() -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let qsb = std::env::var_os("QMAKE")
        .map(PathBuf::from)
        .and_then(|qmake| qmake.parent().map(|bin| bin.join("qsb")))
        .unwrap_or_else(|| PathBuf::from("qsb"));
    let mut qrc = String::from("<RCC>\n  <qresource prefix=\"/shaders\">\n");
    for shader in SHADERS {
        let source = src.join(shader);
        let committed = src.join(format!("{shader}.qsb"));
        println!("cargo::rerun-if-changed={}", source.display());
        println!("cargo::rerun-if-changed={}", committed.display());
        let built = out.join(format!("{shader}.qsb"));
        let compiled = std::process::Command::new(&qsb)
            .args(["--glsl", "100es,120,150", "--hlsl", "50", "--msl", "12", "-o"])
            .arg(&built)
            .arg(&source)
            .status()
            .is_ok_and(|s| s.success());
        let path = if compiled { built } else { committed };
        let path = path.display().to_string().replace('\\', "/");
        qrc.push_str(&format!("    <file alias=\"{shader}.qsb\">{path}</file>\n"));
    }
    qrc.push_str("  </qresource>\n</RCC>\n");
    let qrc_path = out.join("shaders.qrc");
    write_if_changed(&qrc_path, qrc);
    qrc_path
}

fn find_lrelease() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("LRELEASE") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(qmake) = std::env::var_os("QMAKE").map(PathBuf::from)
        && let Some(bin_dir) = qmake.parent()
    {
        for name in ["lrelease", "lrelease-qt6"] {
            let candidate = bin_dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for candidate in [
        "/usr/lib/qt6/bin/lrelease",
        "/usr/lib64/qt6/bin/lrelease",
        "/usr/bin/lrelease-qt6",
        "/usr/bin/lrelease",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn offered_languages(manifest_dir: &Path) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let i18n_rs = manifest_dir.join("src").join("i18n.rs");
    println!("cargo:rerun-if-changed={}", i18n_rs.display());
    let Ok(src) = fs::read_to_string(&i18n_rs) else {
        return set;
    };
    let Some((_, rest)) = src.split_once("pub const LANGUAGES") else {
        return set;
    };
    let Some((table, _)) = rest.split_once("];") else {
        return set;
    };
    for part in table.split('(').skip(1) {
        if let Some(after_quote) = part.strip_prefix('"')
            && let Some((code, _)) = after_quote.split_once('"')
            && !code.is_empty()
        {
            set.insert(code.to_string());
        }
    }
    set
}

fn compile_translations() {
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let manifest_path = Path::new(&manifest_dir);
    let offered = offered_languages(manifest_path);
    let i18n_dir = manifest_path.join("i18n");
    println!("cargo:rerun-if-changed={}", i18n_dir.display());
    let Ok(entries) = fs::read_dir(&i18n_dir) else {
        return;
    };
    let ts_files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("ts"))
        .collect();
    for ts in &ts_files {
        println!("cargo:rerun-if-changed={}", ts.display());
    }
    if ts_files.is_empty() {
        return;
    }
    let Some(lrelease) = find_lrelease() else {
        println!(
            "cargo:warning=lrelease not found; skipping .qm compilation (install qt6-l10n-tools or set LRELEASE)"
        );
        return;
    };
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let out_path = PathBuf::from(out_dir);
    let profile_dir = out_path.ancestors().nth(3).map(Path::to_path_buf);

    for ts in ts_files {
        let Some(stem) = ts.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(lang) = stem.strip_prefix("g7link_") else {
            continue;
        };
        if !offered.contains(lang) {
            continue;
        }
        let qm_name = format!("{stem}.qm");
        let out_qm = out_path.join(&qm_name);
        let compiled = std::process::Command::new(&lrelease)
            .arg("-silent")
            .arg(&ts)
            .arg("-qm")
            .arg(&out_qm)
            .status()
            .is_ok_and(|s| s.success());
        if compiled && let Some(ref target_dir) = profile_dir {
            let _ = fs::copy(&out_qm, target_dir.join(&qm_name));
        }
    }
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(linux_ffmpeg_video)");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        build_linux_video_codec();
    }
    for qml in QML.iter().chain(QML_SINGLETONS.iter()) {
        println!("cargo:rerun-if-changed={qml}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        compile_translations();
    }
    let module = QmlModule::new("app.g7link")
        .qml_files(QML.iter().map(|f| QmlFile::from(*f)))
        .qml_files(QML_SINGLETONS.iter().map(|f| QmlFile::from(*f).singleton(true)));
    let mut builder = CxxQtBuilder::new_qml_module(module).qt_module("Quick");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        builder = builder.qt_module("Widgets");
    }
    builder
        .include_dir("cpp")
        .cpp_files([
            "cpp/app_helpers.cpp",
            "cpp/native_window.h",
            "cpp/native_window.cpp",
            "cpp/video_view.h",
            "cpp/video_view.cpp",
        ])
        .qrc(fonts_qrc())
        .qrc(shaders_qrc())
        .files([
            "src/bridge/native.rs",
            "src/bridge/app.rs",
            "src/bridge/calls.rs",
            "src/bridge/devices.rs",
            "src/bridge/history.rs",
            "src/bridge/media.rs",
            "src/bridge/messages.rs",
            "src/bridge/mirror.rs",
            "src/bridge/notifications.rs",
            "src/bridge/pairing.rs",
            "src/bridge/phone_mic.rs",
            "src/bridge/photo_backup.rs",
            "src/bridge/photos.rs",
            "src/bridge/prefs.rs",
            "src/bridge/timeline.rs",
            "src/bridge/transfers.rs",
            "src/bridge/webcam.rs",
        ])
        .build();

    println!("cargo:rustc-link-lib=Qt6Quick");
    println!("cargo:rustc-link-lib=Qt6Widgets");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:$ORIGIN/lib:$ORIGIN/../lib:$ORIGIN/../lib/g7link");
    println!("cargo:rerun-if-changed=cpp/app_helpers.h");
    println!("cargo:rerun-if-changed=cpp/app_helpers.cpp");
    println!("cargo:rerun-if-changed=cpp/qt_thread_sync.h");
    println!("cargo:rerun-if-changed=cpp/native_window.h");
    println!("cargo:rerun-if-changed=cpp/native_window.cpp");
    println!("cargo:rerun-if-changed=cpp/video_view.h");
    println!("cargo:rerun-if-changed=cpp/video_view.cpp");
}

fn build_linux_video_codec() {
    println!("cargo::rerun-if-changed=cpp/linux_video_codec.h");
    println!("cargo::rerun-if-changed=cpp/linux_video_codec.cpp");
    println!("cargo::rerun-if-changed=cpp/linux_video_builtin.h");
    println!("cargo::rerun-if-changed=cpp/linux_video_dispatch.cpp");
    let libraries =
        ["libavcodec", "libavutil"].map(|name| pkg_config::Config::new().cargo_metadata(false).probe(name));
    let [Ok(codec), Ok(util)] = libraries else { return };
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").define("G7LINK_BUILTIN_VIDEO", None).file("cpp/linux_video_codec.cpp");
    for path in codec.include_paths.iter().chain(&util.include_paths) {
        build.include(path);
    }
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("cpp/linux_video_dispatch.cpp")
        .compile("g7link_linux_video_dispatch");
    build.compile("g7link_linux_video_codec");
    println!("cargo::rustc-link-lib=dl");
    println!("cargo::rustc-cfg=linux_ffmpeg_video");
}
