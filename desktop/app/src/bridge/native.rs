// SPDX-License-Identifier: GPL-3.0-or-later
//! Qt calls that cxx-qt-lib doesn't wrap (see `cpp/app_helpers.cpp`).

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("app_helpers.h");
        include!("video_view.h");
        include!("cxx-qt-lib/qguiapplication.h");
        include!("cxx-qt-lib/qqmlapplicationengine.h");
        type QGuiApplication = cxx_qt_lib::QGuiApplication;
        type QQmlApplicationEngine = cxx_qt_lib::QQmlApplicationEngine;

        fn create_qt_app() -> UniquePtr<QGuiApplication>;

        /// The latest picture of a video stream (32-bit BGRX rows), for
        /// `VideoView`s showing it. Any thread.
        fn video_frame(stream: &str, width: u32, height: u32, bgrx: &[u8]);
        /// The latest picture of a video stream as decoded (NV12: `width` x
        /// `height` visible in rows of `stride` bytes, the UV plane
        /// `plane_rows` rows in), which the GPU turns into RGB while drawing
        /// it. Any thread.
        #[allow(clippy::too_many_arguments)]
        fn video_frame_nv12(
            stream: &str,
            width: u32,
            height: u32,
            stride: u32,
            plane_rows: u32,
            nv12: &[u8],
            bt601: bool,
            full_range: bool,
        );
        /// The stream ended: its views go blank. Any thread.
        fn video_clear(stream: &str);

        /// Must run before `QGuiApplication` is created. Qt's messages go to
        /// `qml.log` in `log_dir`, beside the app's own log.
        fn prepare_qt(log_dir: &str);
        /// Like `prepare_qt`, for `--ui-check`: Qt's messages go to stderr
        /// instead of the log file, and QML warnings and errors are counted.
        fn prepare_qt_for_ui_check();

        /// Draws the app at `percent` of its normal size. Before the app object is
        /// made. `QT_SCALE_FACTOR` set by hand wins.
        fn set_ui_scale(percent: i32);
        /// How many QML warnings and errors there were since
        /// `prepare_qt_for_ui_check`.
        fn qml_problem_count() -> i32;
        fn add_app_icon_image(size: i32, rgba: &[u8]);
        fn apply_app_icon();
        fn keep_running_without_windows();
        /// Registers the fonts compiled in under `:/fonts/`; returns how
        /// many loaded.
        fn load_bundled_fonts() -> i32;
        /// Clears Qt pixmap and QML caches.
        fn trim_memory_caches();
        /// Records the active QML engine so `trim_memory_caches` can run GC and
        /// clear component caches.
        fn register_qml_app_engine(engine: Pin<&mut QQmlApplicationEngine>);
        /// Starts a native OS drag-and-drop operation for a local file or text.
        fn start_external_drag(file_path: &str, text_fallback: &str) -> bool;

        /// Installs the G7 Link and Qt translations for `language` (a
        /// locale name, or "" for the system's), looking in `dir` for the
        /// .qm files and replacing those installed before. Run after the
        /// `QGuiApplication` exists; before any QML engine loads, or later to
        /// retranslate the open windows. Returns true when G7 Link's own
        /// translation was installed.
        fn install_translator(dir: &str, language: &str) -> bool;
        /// The languages the user prefers, in order (`de-DE`, `en-US`).
        fn system_ui_languages() -> Vec<String>;

        /// `text` in the installed language (`QCoreApplication::translate`).
        fn translate(context: &str, text: &str) -> String;

        /// Decodes image bytes into a byte vector: 4-byte width (LE), 4-byte height (LE), followed by ARGB32 bytes.
        fn decode_image(bytes: &[u8]) -> Vec<u8>;

        #[allow(clippy::too_many_arguments)]
        fn linux_tray_setup(
            icon_path: &str,
            tooltip: &str,
            open_label: &str,
            command_palette_label: &str,
            shelf_label: &str,
            take_photo_label: &str,
            scan_document_label: &str,
            find_phone_label: &str,
            open_link_label: &str,
            pause_notifications_label: &str,
            quit_label: &str,
        );

        fn linux_tray_set_tooltip(tooltip: &str);

        fn linux_tray_set_pause_label(label: &str);

        fn linux_tray_set_find_phone_enabled(enabled: bool);

        fn linux_tray_teardown();

        fn linux_tray_wait_action(timeout_ms: u32) -> String;

        fn linux_work_area(x: &mut i32, y: &mut i32, w: &mut i32, h: &mut i32) -> bool;

        fn linux_screen_geometry(x: &mut i32, y: &mut i32, w: &mut i32, h: &mut i32) -> bool;

        fn linux_set_activation_token(token: &str);

        fn linux_clipboard_sequence() -> u64;

        fn linux_clipboard_wait_change(last_seq: u64, timeout_ms: u32) -> u64;

        #[allow(clippy::too_many_arguments)]
        fn linux_clipboard_read(
            check_privacy: bool,
            is_private: &mut bool,
            has_files: &mut bool,
            text: &mut String,
            html: &mut String,
            rtf: &mut String,
            png_bytes: &mut Vec<u8>,
        ) -> bool;

        fn linux_clipboard_read_files() -> Vec<String>;

        fn linux_clipboard_write_text(text: &str, html: &str, rtf: &str, is_private: bool) -> bool;

        fn linux_clipboard_write_image_png(png: &[u8]) -> bool;

        fn linux_clipboard_write_files(paths: &[String]) -> bool;
    }
}
