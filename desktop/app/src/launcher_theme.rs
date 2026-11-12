// SPDX-License-Identifier: GPL-3.0-or-later
//! "Match G7 Linux": the colors of the G7 Linux launcher's current light and
//! dark themes, read (never written) from its files and followed live.
//!
//! The launcher keeps its settings in `$XDG_CONFIG_HOME/g7linux/settings.json`
//! (JSON with comments), where `theme.light.name` and `theme.dark.name` name a
//! theme; unset, they're its own `g7linux-light` and `g7linux-dark`, which are
//! G7's colors. A theme is `<name>.toml` in `$XDG_DATA_HOME/g7linux/themes` or
//! one of the `$XDG_DATA_DIRS/g7linux/themes`, the first found winning, and may
//! inherit another. Only its core colors and accents are read: the app maps
//! them onto its own roles (Theme.qml), so the layout never changes.

use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

/// The colors read from a theme: `[colors.core]`, then `[colors.accents]`.
const KEYS: [&str; 12] = [
    "background",
    "foreground",
    "border",
    "accent",
    "blue",
    "green",
    "magenta",
    "orange",
    "purple",
    "red",
    "yellow",
    "cyan",
];
/// How many of [`KEYS`] are core colors.
const CORE_KEYS: usize = 4;
/// The launcher's own themes, which every theme falls back to: `(light, dark)`
/// for each of [`KEYS`].
const BUILTIN_COLORS: [(&str, &str); 12] = [
    ("#fafafb", "#18181b"),
    ("#1c1c1f", "#f2f2f3"),
    ("#dcdce1", "#303035"),
    ("#7d5ca9", "#b292d0"),
    ("#3557c2", "#7aa2f7"),
    ("#2d7a4d", "#6cc08b"),
    ("#7d5ca9", "#b292d0"),
    ("#b8661f", "#e8a05c"),
    ("#7d5ca9", "#b292d0"),
    ("#b0432b", "#e0785f"),
    ("#8f6f1f", "#d9bf72"),
    ("#2f7480", "#6ab7c2"),
];
/// Larger settings or theme files are not read.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// How deep inheritance is followed (a loop ends here too).
const MAX_INHERIT_DEPTH: usize = 8;
/// How many of `$XDG_DATA_DIRS` are searched for themes.
const MAX_DATA_DIRS: usize = 16;
/// Longest theme name looked up (it's a file name).
const MAX_ID_LEN: usize = 128;

type Rgb = [u8; 3];

/// A theme as the app uses it: its name, whether it's dark, and its colors
/// (`None`: the launcher's own, which are G7's).
#[derive(Debug, Clone, PartialEq)]
struct LauncherTheme {
    name: String,
    dark: bool,
    colors: Option<[Rgb; 12]>,
}

impl LauncherTheme {
    fn builtin(dark: bool) -> Self {
        let name = if dark { "G7 Linux Dark" } else { "G7 Linux Light" };
        LauncherTheme { name: name.to_owned(), dark, colors: None }
    }

    fn to_json(&self) -> Value {
        let colors = self.colors.map(colors_json);
        json!({ "name": self.name, "dark": self.dark, "colors": colors })
    }
}

fn colors_json(colors: [Rgb; 12]) -> Value {
    let mut map = Map::new();
    for (key, rgb) in KEYS.iter().zip(colors) {
        map.insert((*key).to_owned(), Value::from(hex(rgb)));
    }
    Value::Object(map)
}

/// A color in a theme file: a color, or another of the theme's colors by
/// index in [`KEYS`], optionally see-through over the theme's background.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ColorSpec {
    value: ColorValue,
    opacity: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ColorValue {
    Rgb(Rgb),
    Ref(usize),
}

/// What a theme file says.
#[derive(Debug, Clone, PartialEq)]
struct ThemeFile {
    name: String,
    dark: bool,
    inherits: String,
    colors: [Option<ColorSpec>; 12],
}

/// The launcher's light and dark themes as JSON for QML: `{ light, dark }`,
/// each `{ name, dark, colors }` with `colors` null for G7's own.
pub fn current_json() -> String {
    let settings = dirs::config_dir().and_then(|dir| read_capped(&dir.join("g7linux/settings.json")));
    let (light, dark) = theme_names(settings.as_deref());
    let dirs = theme_dirs();
    let load = |id: &str| load_theme(&dirs, id);
    let light = resolve(&light, false, &load);
    let dark = resolve(&dark, true, &load);
    json!({ "light": light.to_json(), "dark": dark.to_json() }).to_string()
}

/// Calls `on_change` with [`current_json`] whenever it changes: the
/// launcher's settings or a theme file was written.
pub fn watch(on_change: impl Fn(String) + Send + 'static) {
    let last = RefCell::new(current_json());
    crate::sys::dir_watch::watch("launcher-theme", watched_dirs, move || {
        let now = current_json();
        if *last.borrow() != now {
            *last.borrow_mut() = now.clone();
            on_change(now);
        }
    });
}

fn watched_dirs() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = dirs::config_dir().map(|dir| dir.join("g7linux")).into_iter().collect();
    found.extend(theme_dirs());
    found
}

/// The launcher's theme folders, in the order it searches them.
fn theme_dirs() -> Vec<PathBuf> {
    let user = dirs::data_dir().map(|dir| dir.join("g7linux/themes"));
    let mut found: Vec<PathBuf> = user.into_iter().collect();
    let system = std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty());
    let system = system.unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in std::env::split_paths(&system).filter(|dir| dir.is_absolute()).take(MAX_DATA_DIRS) {
        let themes = dir.join("g7linux/themes");
        if !found.contains(&themes) {
            found.push(themes);
        }
    }
    found
}

/// A file's text, when it's a regular file of at most [`MAX_FILE_BYTES`] in UTF-8.
fn read_capped(path: &Path) -> Option<String> {
    // Non-blocking, so a FIFO put in its place can't hang the read.
    let file: File = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// A theme file's text by theme name, from the first folder that has it.
fn load_theme(dirs: &[PathBuf], id: &str) -> Option<String> {
    // The name becomes a file name: nothing that could leave the folder.
    let forbidden = id.starts_with('.') || id.contains(['/', '\\', '\0']);
    if id.is_empty() || id.len() > MAX_ID_LEN || forbidden {
        return None;
    }
    dirs.iter().find_map(|dir| read_capped(&dir.join(format!("{id}.toml"))))
}

/// The light and dark theme names from the launcher's settings (JSON with
/// comments), its own themes where unset.
fn theme_names(settings: Option<&str>) -> (String, String) {
    let value: Option<Value> = settings.and_then(|text| serde_json::from_str(&strip_jsonc(text)).ok());
    let name = |slot: &str, fallback: &str| {
        let pointer = format!("/theme/{slot}/name");
        let set = value.as_ref().and_then(|v| v.pointer(&pointer)).and_then(Value::as_str);
        set.map(str::trim).filter(|name| !name.is_empty()).unwrap_or(fallback).to_owned()
    };
    (name("light", "g7linux-light"), name("dark", "g7linux-dark"))
}

/// JSON with `//` and `/* */` comments and trailing commas, as plain JSON.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => while chars.next_if(|&next| next != '\n').is_some() {},
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for next in chars.by_ref() {
                    if last == '*' && next == '/' {
                        break;
                    }
                    last = next;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    remove_trailing_commas(&out)
}

fn remove_trailing_commas(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in json.char_indices() {
        if in_string {
            in_string = escaped || c != '"';
            escaped = !escaped && c == '\\';
        } else if c == '"' {
            in_string = true;
        } else if c == ',' && json[i + 1..].trim_start().starts_with(['}', ']']) {
            continue;
        }
        out.push(c);
    }
    out
}

fn builtin_dark(id: &str) -> Option<bool> {
    // The launcher still takes the names its themes had as Vicinae.
    match id {
        "g7linux-dark" | "vicinae-dark" => Some(true),
        "g7linux-light" | "vicinae-light" => Some(false),
        _ => None,
    }
}

fn builtin_colors(dark: bool) -> [Rgb; 12] {
    BUILTIN_COLORS.map(|(light, dark_color)| {
        let color = if dark { dark_color } else { light };
        parse_hex(color).unwrap_or_default()
    })
}

/// The theme named `id` in the launcher's light or dark (`dark_slot`) slot.
fn resolve(id: &str, dark_slot: bool, load: &dyn Fn(&str) -> Option<String>) -> LauncherTheme {
    if let Some(dark) = builtin_dark(id) {
        return LauncherTheme::builtin(dark);
    }
    // A theme the launcher can't find or read leaves it on its own: G7's colors.
    let Some(file) = load(id).as_deref().and_then(parse_theme) else {
        return LauncherTheme::builtin(dark_slot);
    };
    let colors = resolve_colors(&file, load, 0);
    LauncherTheme { name: file.name, dark: file.dark, colors: Some(colors) }
}

/// A theme's colors: its own, over those of the theme it inherits.
fn resolve_colors(file: &ThemeFile, load: &dyn Fn(&str) -> Option<String>, depth: usize) -> [Rgb; 12] {
    let inherited = match builtin_dark(&file.inherits) {
        Some(dark) => builtin_colors(dark),
        None => {
            let parent = if depth < MAX_INHERIT_DEPTH { load(&file.inherits) } else { None };
            match parent.as_deref().and_then(parse_theme) {
                Some(parent) => resolve_colors(&parent, load, depth + 1),
                // Unknown, or too deep: the launcher's own theme of its kind.
                None => builtin_colors(file.dark),
            }
        }
    };
    std::array::from_fn(|i| color_at(file, &inherited, i, 0))
}

/// Color `i` of a theme: its own (following references within the theme),
/// else the inherited one.
fn color_at(file: &ThemeFile, inherited: &[Rgb; 12], i: usize, hops: usize) -> Rgb {
    let Some(spec) = file.colors[i] else { return inherited[i] };
    let rgb = match spec.value {
        ColorValue::Rgb(rgb) => rgb,
        ColorValue::Ref(_) if hops >= KEYS.len() => return inherited[i],
        ColorValue::Ref(target) => color_at(file, inherited, target, hops + 1),
    };
    match spec.opacity {
        // See-through: drawn over the theme's background, as the launcher does.
        Some(opacity) if i != 0 => blend(rgb, color_at(file, inherited, 0, hops + 1), opacity),
        _ => rgb,
    }
}

fn blend(top: Rgb, under: Rgb, opacity: f64) -> Rgb {
    let mix = |c: usize| (f64::from(top[c]) * opacity + f64::from(under[c]) * (1.0 - opacity)).round() as u8;
    [mix(0), mix(1), mix(2)]
}

/// A theme file, when it's one the launcher would load.
fn parse_theme(text: &str) -> Option<ThemeFile> {
    let doc: toml::Table = toml::from_str(text).ok()?;
    let meta = doc.get("meta")?.as_table()?;
    let name = meta.get("name")?.as_str()?.trim().to_owned();
    // The launcher doesn't load a theme without a description.
    meta.get("description")?.as_str()?;
    let dark = meta.get("variant")?.as_str()? != "light";
    let inherits = match meta.get("inherits") {
        Some(parent) => parent.as_str()?.trim().to_owned(),
        None => String::from(if dark { "g7linux-dark" } else { "g7linux-light" }),
    };
    let colors = doc.get("colors").and_then(toml::Value::as_table);
    let section = |name: &str| colors.and_then(|c| c.get(name)).and_then(toml::Value::as_table);
    let (core, accents) = (section("core"), section("accents"));
    let colors = std::array::from_fn(|i| {
        let table = if i < CORE_KEYS { core } else { accents };
        table.and_then(|t| t.get(KEYS[i])).and_then(parse_color)
    });
    Some(ThemeFile { name, dark, inherits, colors })
}

/// `"#rrggbb"`, `"colors.accents.blue"`, or `{ name = …, opacity = 0.5 }`.
fn parse_color(value: &toml::Value) -> Option<ColorSpec> {
    if let Some(text) = value.as_str() {
        return Some(ColorSpec { value: parse_color_name(text)?, opacity: None });
    }
    let table = value.as_table()?;
    let value = parse_color_name(table.get("name")?.as_str()?)?;
    let opacity = table.get("opacity").and_then(toml::Value::as_float).map(|o| o.clamp(0.0, 1.0));
    Some(ColorSpec { value, opacity })
}

fn parse_color_name(text: &str) -> Option<ColorValue> {
    let text = text.trim();
    if let Some(key) = text.strip_prefix("colors.core.") {
        return KEYS[..CORE_KEYS].iter().position(|k| *k == key).map(ColorValue::Ref);
    }
    if let Some(key) = text.strip_prefix("colors.accents.") {
        return KEYS[CORE_KEYS..].iter().position(|k| *k == key).map(|i| ColorValue::Ref(CORE_KEYS + i));
    }
    parse_hex(text).map(ColorValue::Rgb)
}

/// `#rgb`, `#rrggbb` or `#aarrggbb` (its alpha dropped), with or without the `#`.
fn parse_hex(text: &str) -> Option<Rgb> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    match hex.len() {
        3 => {
            let nibble = |at: usize| u8::from_str_radix(&hex[at..=at], 16).ok().map(|n| n * 17);
            Some([nibble(0)?, nibble(1)?, nibble(2)?])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?]),
        8 => Some([byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

fn hex([r, g, b]: Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCHA: &str = r##"
[meta]
name = "Catppuccin Mocha"
description = "The original"
variant = "dark"

[colors.core]
background = "#1E1E2E"
foreground = "#CDD6F4"
border = "#424265"
accent = "colors.accents.blue"

[colors.accents]
blue = "#89B4FA"
green = "#A6E3A1"
magenta = "#F5C2E7"
orange = "#FAB387"
purple = "#CBA6F7"
red = "#F38BA8"
yellow = "#F9E2AF"
cyan = "#94E2D5"

[colors.list.item.selection]
background = "#41425d"
"##;

    fn files(list: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |id: &str| list.iter().find(|(name, _)| *name == id).map(|(_, text)| (*text).to_owned())
    }

    fn colors(theme: &LauncherTheme) -> Vec<String> {
        theme.colors.expect("colors").iter().map(|rgb| hex(*rgb)).collect()
    }

    #[test]
    fn reads_theme_names_from_settings_with_comments() {
        let settings = r#"{
            // The launcher's look.
            "theme": {
                "light": { "name": "catppuccin-latte", /* inline */ "icon_theme": "auto", },
                "dark": { "name": "catppuccin-mocha" },
            },
            "url": "https://example.org/a//b",
        }"#;
        let expected = ("catppuccin-latte".to_owned(), "catppuccin-mocha".to_owned());
        assert_eq!(theme_names(Some(settings)), expected);
    }

    #[test]
    fn missing_settings_and_names_are_the_launchers_own_themes() {
        let own = ("g7linux-light".to_owned(), "g7linux-dark".to_owned());
        assert_eq!(theme_names(None), own);
        assert_eq!(theme_names(Some("{}")), own);
        assert_eq!(theme_names(Some("not json")), own);
        assert_eq!(theme_names(Some(r#"{ "theme": { "dark": { "name": " " } } }"#)), own);
        let only_dark = theme_names(Some(r#"{ "theme": { "dark": { "name": "nord" } } }"#));
        assert_eq!(only_dark, ("g7linux-light".to_owned(), "nord".to_owned()));
    }

    #[test]
    fn strings_keep_what_looks_like_comments() {
        let text = r#"{ "a": "x // y", "b": "q\"/*", "c": [1, 2,], }"#;
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value, json!({ "a": "x // y", "b": "q\"/*", "c": [1, 2] }));
    }

    #[test]
    fn the_launchers_own_and_unknown_themes_are_g7() {
        let load = files(&[]);
        assert_eq!(resolve("g7linux-dark", true, &load), LauncherTheme::builtin(true));
        assert_eq!(resolve("vicinae-light", false, &load), LauncherTheme::builtin(false));
        let unknown = resolve("no-such-theme", true, &load);
        assert_eq!(unknown, LauncherTheme::builtin(true));
        assert_eq!(unknown.to_json()["colors"], Value::Null);
        assert_eq!(unknown.to_json()["name"], "G7 Linux Dark");
    }

    #[test]
    fn maps_a_theme_files_colors() {
        let theme = resolve("catppuccin-mocha", true, &files(&[("catppuccin-mocha", MOCHA)]));
        assert_eq!(theme.name, "Catppuccin Mocha");
        assert!(theme.dark);
        assert_eq!(
            colors(&theme).join(" "),
            "#1e1e2e #cdd6f4 #424265 #89b4fa #89b4fa #a6e3a1 #f5c2e7 #fab387 #cba6f7 #f38ba8 #f9e2af #94e2d5"
        );
        let json = theme.to_json();
        assert_eq!(json["colors"]["accent"], "#89b4fa");
        assert_eq!(json["colors"]["cyan"], "#94e2d5");
    }

    #[test]
    fn missing_colors_come_from_the_inherited_theme() {
        const CHILD: &str = r##"
[meta]
name = "Child"
description = "Inherits Mocha"
variant = "dark"
inherits = "catppuccin-mocha"

[colors.core]
accent = "#123"
border = { name = "colors.core.foreground", opacity = 0.5 }
"##;
        const PARTIAL: &str = r##"
[meta]
name = "Partial"
description = "Only a background"
variant = "light"

[colors.core]
background = "#fdf6e3"
"##;
        let load = files(&[("child", CHILD), ("catppuccin-mocha", MOCHA), ("partial", PARTIAL)]);
        let child = colors(&resolve("child", true, &load));
        assert_eq!(child[0], "#1e1e2e", "the parent's background");
        assert_eq!(child[3], "#112233", "its own accent");
        // Half the foreground over the background.
        assert_eq!(child[2], "#767a91");
        assert_eq!(child[4], "#89b4fa", "the parent's accents");

        let partial = resolve("partial", true, &load);
        assert!(!partial.dark);
        let partial = colors(&partial);
        assert_eq!(partial[0], "#fdf6e3");
        assert_eq!(partial[1], "#1c1c1f", "the launcher's own light foreground");
    }

    #[test]
    fn inheritance_loops_end() {
        const LOOP: &str = r##"
[meta]
name = "Loop"
description = "Inherits itself"
variant = "dark"
inherits = "loop"

[colors.core]
background = "#000000"
accent = "colors.core.accent"
"##;
        let theme = colors(&resolve("loop", true, &files(&[("loop", LOOP)])));
        assert_eq!(theme[0], "#000000");
        assert_eq!(theme[3], "#b292d0", "a reference to itself falls back");
    }

    #[test]
    fn rejects_what_the_launcher_would_not_load() {
        assert!(parse_theme("not toml [").is_none());
        assert!(parse_theme("[meta]\nname = \"No variant\"\ndescription = \"x\"\n").is_none());
        assert!(parse_theme("[colors.core]\nbackground = \"#000000\"\n").is_none());
        assert_eq!(parse_hex("#1e1e2e"), Some([0x1e, 0x1e, 0x2e]));
        assert_eq!(parse_hex("ff1e1e2e"), Some([0x1e, 0x1e, 0x2e]));
        assert_eq!(parse_hex("#abc"), Some([0xaa, 0xbb, 0xcc]));
        for bad in ["", "#12", "#12345", "red", "#+1+1+1", "#ééé"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn theme_names_never_leave_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir(&themes).unwrap();
        std::fs::write(themes.join("nord.toml"), "x").unwrap();
        std::fs::write(dir.path().join("outside.toml"), "x").unwrap();
        let dirs = [themes.clone()];
        assert_eq!(load_theme(&dirs, "nord").as_deref(), Some("x"));
        for id in ["../outside", "", ".hidden", "a/b", "a\\b"] {
            assert_eq!(load_theme(&dirs, id), None, "{id}");
        }
        // A folder named like a theme isn't one.
        std::fs::create_dir(themes.join("folder.toml")).unwrap();
        assert_eq!(load_theme(&dirs, "folder"), None);
    }

    #[test]
    fn large_files_are_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.toml");
        std::fs::write(&path, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert_eq!(read_capped(&path), None);
        std::fs::write(&path, "small").unwrap();
        assert_eq!(read_capped(&path).as_deref(), Some("small"));
    }
}
