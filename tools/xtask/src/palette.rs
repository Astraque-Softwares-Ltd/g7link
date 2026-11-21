// SPDX-License-Identifier: GPL-3.0-or-later
//! The agent timeline's palette (`themes.quiet.agents` in the token file):
//! its checks, and the files the phone and the launcher read it from.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::{contrast_ratio, is_hex_color, parse_rgb, strip_docs};

/// Every render-crate token class, by its word.
pub const SYNTAX_CLASSES: &[&str] = &[
    "plain",
    "comment",
    "keyword",
    "string",
    "number",
    "constant",
    "type",
    "function",
    "variable",
    "operator",
    "punctuation",
    "tag",
    "attribute",
    "inserted",
    "deleted",
    "dir",
    "file",
    "pos",
    "error",
    "warn",
    "ok",
];

/// Every tool class, by its word.
pub const TOOL_CLASSES: &[&str] =
    &["command", "edit", "read", "search", "web", "mcp", "agent", "tasks", "other", "plan"];

/// Every tool line status that has a colour.
const TOOL_STATUSES: &[&str] = &["ok", "failed", "running", "asking", "denied", "stopped"];

/// Roles that may be the accent: links, and lavender drawn as a glyph, a
/// rule or an alert's word.
const ACCENT_ALLOWED: &[&str] = &["prose.link", "prose.headingRule", "prose.alertImportant", "tool.agent"];

/// Text needs 4.5:1; a glyph or a rule 3:1 (WCAG 2.1, 1.4.3 and 1.4.11).
const TEXT_RATIO: f64 = 4.5;
const GLYPH_RATIO: f64 = 3.0;
/// Pairs that must be told apart without hue differ at least this much in
/// OKLab lightness.
const MIN_LIGHTNESS_GAP: f64 = 0.06;

/// The sections a theme has, in the order they are generated.
const SECTIONS: &[&str] = &["ground", "steps", "syntax", "diff", "status", "toolStatus", "tool", "prose"];

/// A colour: a hex, or a wash drawn over a ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint<'a> {
    Solid(&'a str),
    Wash { color: &'a str, alpha: f64 },
}

fn paint<'a>(value: &'a Value, name: &str) -> Result<Paint<'a>> {
    if let Some(hex) = value.as_str() {
        if !is_hex_color(hex) {
            bail!("{name}: {hex:?} is not a #RRGGBB color");
        }
        return Ok(Paint::Solid(hex));
    }
    let color = value.get("color").and_then(Value::as_str).with_context(|| format!("{name}: no color"))?;
    let alpha = value.get("alpha").and_then(Value::as_f64).with_context(|| format!("{name}: no alpha"))?;
    if !is_hex_color(color) || !(alpha > 0.0 && alpha <= 1.0) {
        bail!("{name}: {color:?} at {alpha} is not a wash");
    }
    Ok(Paint::Wash { color, alpha })
}

/// `paint` as a solid colour over `ground`.
fn over(paint: Paint<'_>, ground: &str) -> Result<String> {
    match paint {
        Paint::Solid(hex) => Ok(hex.to_owned()),
        Paint::Wash { color, alpha } => {
            let (fr, fg, fb) = parse_rgb(color)?;
            let (br, bg, bb) = parse_rgb(ground)?;
            let mix = |f: u8, b: u8| (f64::from(b) + alpha * (f64::from(f) - f64::from(b))).round() as u8;
            Ok(format!("#{:02X}{:02X}{:02X}", mix(fr, br), mix(fg, bg), mix(fb, bb)))
        }
    }
}

/// OKLab lightness, 0 to 1.
pub fn oklab_l(hex: &str) -> Result<f64> {
    let (r, g, b) = parse_rgb(hex)?;
    let lin = |c: u8| {
        let s = f64::from(c) / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    Ok(0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_042 * s)
}

/// One theme of the palette, read and checked.
struct Theme<'a> {
    name: String,
    roles: &'a Map<String, Value>,
}

impl<'a> Theme<'a> {
    fn section(&self, section: &str) -> Result<&'a Map<String, Value>> {
        self.roles
            .get(section)
            .and_then(Value::as_object)
            .with_context(|| format!("{}: missing {section}", self.name))
    }

    /// The paint at `section.key`.
    fn get(&self, path: &str) -> Result<Paint<'a>> {
        let (section, key) = path.split_once('.').with_context(|| format!("{path}: not section.key"))?;
        let value =
            self.section(section)?.get(key).with_context(|| format!("{}: missing {path}", self.name))?;
        paint(value, &format!("{}.{path}", self.name))
    }

    fn ground(&self, key: &str) -> Result<&'a str> {
        match self.get(&format!("ground.{key}"))? {
            Paint::Solid(hex) => Ok(hex),
            Paint::Wash { .. } => bail!("{}: ground.{key} is a wash", self.name),
        }
    }

    fn need(&self, fg: &str, bg: &str, ratio: f64, what: &str) -> Result<()> {
        let got = contrast_ratio(fg, bg)?;
        if got < ratio {
            bail!("{}: {what} ({fg} on {bg}) has contrast {got:.2}:1 (< {ratio}:1)", self.name);
        }
        Ok(())
    }
}

/// Checks `themes.quiet.agents` against the variants it is drawn on.
pub fn check(tokens: &Value) -> Result<()> {
    let quiet = tokens.pointer("/themes/quiet").context("missing quiet theme")?;
    let agents = quiet.get("agents").and_then(Value::as_object).context("missing quiet.agents")?;
    let glyph_only: Vec<&str> = agents
        .get("glyphOnly")
        .and_then(Value::as_array)
        .context("agents: missing glyphOnly")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let styles = agents.get("styles").and_then(Value::as_object).context("agents: missing styles")?;
    for (class, style) in styles {
        if !SYNTAX_CLASSES.contains(&class.as_str()) || !matches!(style.as_str(), Some("italic" | "semibold"))
        {
            bail!("agents.styles.{class}: {style} is not a style of a syntax class");
        }
    }
    let pairs = agents.get("mustDiffer").and_then(Value::as_array).context("agents: missing mustDiffer")?;
    for variant in ["dark", "light"] {
        let roles = agents
            .get(variant)
            .and_then(Value::as_object)
            .with_context(|| format!("agents: missing {variant}"))?;
        let palette = quiet
            .pointer(&format!("/variants/{variant}"))
            .and_then(Value::as_object)
            .with_context(|| format!("missing variant {variant}"))?;
        let theme = Theme { name: format!("agents.{variant}"), roles };
        check_theme(&theme, palette, &glyph_only, pairs)?;
    }
    let text_tints =
        quiet.pointer("/tintsText/light").and_then(Value::as_object).context("missing tintsText.light")?;
    let chip =
        quiet.pointer("/variants/light/surfaceContainerHigh").and_then(Value::as_str).unwrap_or("#FFFFFF");
    for (name, value) in text_tints {
        let hex =
            value.as_str().filter(|h| is_hex_color(h)).with_context(|| format!("tintsText.light.{name}"))?;
        let got = contrast_ratio(hex, chip)?;
        if got < TEXT_RATIO {
            bail!("tintsText.light.{name} ({hex} on {chip}) has contrast {got:.2}:1 (< 4.5:1)");
        }
    }
    Ok(())
}

fn check_theme(
    theme: &Theme<'_>,
    palette: &Map<String, Value>,
    glyph_only: &[&str],
    pairs: &[Value],
) -> Result<()> {
    let n = &theme.name;
    for section in SECTIONS {
        for (key, value) in theme.section(section)? {
            paint(value, &format!("{n}.{section}.{key}"))?;
        }
    }
    // The grounds are the theme's own.
    for (key, role) in
        [("surface", "surface"), ("code", "surfaceContainer"), ("chip", "surfaceContainerHigh")]
    {
        let want = palette.get(role).and_then(Value::as_str).unwrap_or_default();
        if theme.ground(key)? != want {
            bail!("{n}: ground.{key} must be the variant's {role} ({want})");
        }
    }
    let (surface, code, chip) = (theme.ground("surface")?, theme.ground("code")?, theme.ground("chip")?);
    let grounds = [surface, code, chip];
    let solid = |path: &str| -> Result<String> { over(theme.get(path)?, code) };

    // Syntax: text on code and on chips.
    let syntax = theme.section("syntax")?;
    for class in SYNTAX_CLASSES {
        let fg = solid(&format!("syntax.{class}")).with_context(|| format!("{n}: syntax.{class}"))?;
        for (bg, ground) in [(code, "ground.code"), (chip, "ground.chip")] {
            theme.need(&fg, bg, TEXT_RATIO, &format!("syntax.{class} on {ground}"))?;
        }
    }
    if syntax.len() != SYNTAX_CLASSES.len() {
        bail!("{n}: syntax has {} classes, the render crate {}", syntax.len(), SYNTAX_CLASSES.len());
    }
    for class in TOOL_CLASSES {
        theme.get(&format!("tool.{class}"))?;
    }
    for status in TOOL_STATUSES {
        theme.get(&format!("toolStatus.{status}"))?;
    }

    // Tool lines, statuses and prose on the surface.
    for section in ["tool", "toolStatus", "status", "prose"] {
        for (key, value) in theme.section(section)? {
            let path = format!("{section}.{key}");
            if path == "status.onNeedsYou" || path == "status.failedText" {
                continue;
            }
            let Paint::Solid(fg) = paint(value, &path)? else { continue };
            let ratio = if glyph_only.contains(&path.as_str()) { GLYPH_RATIO } else { TEXT_RATIO };
            theme.need(fg, surface, ratio, &format!("{path} on ground.surface"))?;
        }
    }
    let needs = solid("status.needsYou")?;
    theme.need(&solid("status.onNeedsYou")?, &needs, TEXT_RATIO, "status.onNeedsYou on status.needsYou")?;
    let failed_chip = over(theme.get("status.failedWash")?, surface)?;
    theme.need(
        &solid("status.failedText")?,
        &failed_chip,
        TEXT_RATIO,
        "status.failedText on status.failedWash",
    )?;

    // Diffs: the text on every wash, and each row's own ink on it, over
    // every ground; the hunk head on its ground.
    let rows = [("diff.addRow", "diff.addSign"), ("diff.delRow", "diff.delSign")];
    for ground in grounds {
        for wash in ["diff.addRow", "diff.delRow", "diff.addWord", "diff.delWord"] {
            let bg = over(theme.get(wash)?, ground)?;
            theme.need(
                &solid("diff.text")?,
                &bg,
                TEXT_RATIO,
                &format!("diff.text on {wash} over {ground}"),
            )?;
        }
        for (row, sign) in rows {
            let bg = over(theme.get(row)?, ground)?;
            theme.need(&solid(sign)?, &bg, TEXT_RATIO, &format!("{sign} on {row} over {ground}"))?;
            theme.need(
                &solid("diff.lineNo")?,
                &bg,
                TEXT_RATIO,
                &format!("diff.lineNo on {row} over {ground}"),
            )?;
        }
        // The added row is the lighter one, in both themes.
        for (added, removed) in [("diff.addRow", "diff.delRow"), ("diff.addWord", "diff.delWord")] {
            let (a, r) =
                (oklab_l(&over(theme.get(added)?, ground)?)?, oklab_l(&over(theme.get(removed)?, ground)?)?);
            if a <= r {
                bail!("{n}: {added} (L {a:.3}) must be lighter than {removed} (L {r:.3}) over {ground}");
            }
        }
    }
    theme.need(
        &solid("diff.hunk")?,
        &solid("diff.hunkGround")?,
        TEXT_RATIO,
        "diff.hunk on diff.hunkGround",
    )?;

    // Pairs told apart by lightness as well as hue (washes over the code ground).
    for pair in pairs {
        let a = pair.get("a").and_then(Value::as_str).context("mustDiffer: missing a")?;
        let b = pair.get("b").and_then(Value::as_str).context("mustDiffer: missing b")?;
        let (la, lb) = (oklab_l(&solid(a)?)?, oklab_l(&solid(b)?)?);
        if (la - lb).abs() < MIN_LIGHTNESS_GAP {
            bail!(
                "{n}: mustDiffer {a} (L {la:.3}) and {b} (L {lb:.3}) are less than {MIN_LIGHTNESS_GAP} apart"
            );
        }
    }

    // Lavender stays the accent's: no other role resolves to it.
    let accent = palette.get("accent").and_then(Value::as_str).unwrap_or_default();
    for section in SECTIONS.iter().filter(|s| **s != "ground") {
        for (key, value) in theme.section(section)? {
            let path = format!("{section}.{key}");
            let hex = match paint(value, &path)? {
                Paint::Solid(hex) | Paint::Wash { color: hex, .. } => hex,
            };
            if hex.eq_ignore_ascii_case(accent) && !ACCENT_ALLOWED.contains(&path.as_str()) {
                bail!("{n}: {path} is the accent ({accent}); only links and lavender glyphs may be");
            }
        }
    }
    Ok(())
}

/// The palette for the launcher, as JSON: the agents section without its
/// notes, and the light text tints.
pub fn render_launcher(tokens: &Value) -> Result<String> {
    let quiet = tokens.pointer("/themes/quiet").context("missing quiet theme")?;
    let mut out = match strip_docs(quiet.get("agents").context("missing quiet.agents")?) {
        Value::Object(map) => map,
        _ => bail!("quiet.agents is not an object"),
    };
    out.insert(
        "tintsTextLight".into(),
        strip_docs(quiet.pointer("/tintsText/light").context("missing tintsText")?),
    );
    out.insert(
        "$source".into(),
        "Generated from docs/design/tokens.json by `cargo xtask tokens`. Do not edit.".into(),
    );
    Ok(serde_json::to_string_pretty(&Value::Object(out))? + "\n")
}

fn kotlin_color(hex: &str) -> String {
    format!("Color(0xFF{})", hex.trim_start_matches('#').to_ascii_uppercase())
}

fn camel(section: &str, key: &str) -> String {
    let mut chars = key.chars();
    let first = chars.next().map(|c| c.to_ascii_uppercase()).into_iter().collect::<String>();
    format!("{section}{first}{}", chars.as_str())
}

/// The palette for the phone: `AgentPalette` per theme, the syntax styles,
/// and G7's tints as the token file has them.
pub fn render_kotlin(tokens: &Value) -> Result<String> {
    let quiet = tokens.pointer("/themes/quiet").context("missing quiet theme")?;
    let agents = quiet.get("agents").context("missing quiet.agents")?;
    let theme = |variant: &str| -> Result<Vec<(String, Value)>> {
        let roles = agents.get(variant).with_context(|| format!("agents: missing {variant}"))?;
        let mut fields = Vec::new();
        for section in SECTIONS {
            let map = roles
                .get(*section)
                .and_then(Value::as_object)
                .with_context(|| format!("missing {section}"))?;
            for (key, value) in map {
                fields.push((camel(section, key), value.clone()));
            }
        }
        Ok(fields)
    };
    let (dark, light) = (theme("dark")?, theme("light")?);
    if dark.iter().map(|f| &f.0).ne(light.iter().map(|f| &f.0)) {
        bail!("agents.dark and agents.light have different roles");
    }
    let mut s = String::from(
        "// SPDX-License-Identifier: GPL-3.0-or-later\n\
         // Generated from docs/design/tokens.json by `cargo xtask tokens`. Do not edit.\n\
         package app.g7link.android.ui.theme\n\
         \n\
         import androidx.compose.runtime.Immutable\n\
         import androidx.compose.ui.graphics.Color\n\
         \n\
         /** A colour drawn at [alpha] over the ground under it. */\n\
         @Immutable\n\
         data class AgentWash(val color: Color, val alpha: Float)\n\
         \n\
         /** The agent timeline's colours for one theme (plans/agent-colour.md). */\n\
         @Immutable\n\
         data class AgentPalette(\n",
    );
    for (name, value) in &dark {
        let ty = if value.is_object() { "AgentWash" } else { "Color" };
        s.push_str(&format!("    val {name}: {ty},\n"));
    }
    s.push_str(") {\n    companion object {\n");
    for (variant, fields) in [("dark", &dark), ("light", &light)] {
        s.push_str(&format!("        val {variant} = AgentPalette(\n"));
        for (name, value) in fields {
            let v = match paint(value, name)? {
                Paint::Solid(hex) => kotlin_color(hex),
                Paint::Wash { color, alpha } => format!("AgentWash({}, {alpha}f)", kotlin_color(color)),
            };
            s.push_str(&format!("            {name} = {v},\n"));
        }
        s.push_str("        )\n");
    }
    s.push_str("    }\n}\n\n/** Font styles by syntax class word: \"italic\" or \"semibold\". */\nval agentSyntaxStyles: Map<String, String> = mapOf(\n");
    let styles = agents.get("styles").and_then(Value::as_object).context("missing styles")?;
    for (class, style) in styles {
        s.push_str(&format!("    \"{class}\" to \"{}\",\n", style.as_str().unwrap_or_default()));
    }
    s.push_str(")\n\n/** G7's tints as docs/design/tokens.json has them; `lightText` passes as text on any light ground. */\nobject G7TintTokens {\n");
    let tint_names = ["blue", "teal", "coral", "pink", "amber", "green", "red", "lavender"];
    let sets = [
        ("dark", "/tints/dark", "/variants/dark/onSurfaceVariant"),
        ("light", "/tints/light", "/variants/light/onSurfaceVariant"),
        ("lightText", "/tintsText/light", "/variants/light/onSurfaceVariant"),
    ];
    for (name, tints, muted) in sets {
        let tints = quiet.pointer(tints).with_context(|| format!("missing {tints}"))?;
        s.push_str(&format!("    val {name} = G7Tints(\n"));
        for tint in tint_names {
            let hex =
                tints.get(tint).and_then(Value::as_str).with_context(|| format!("{name}: missing {tint}"))?;
            s.push_str(&format!("        {tint} = {},\n", kotlin_color(hex)));
        }
        let muted = quiet.pointer(muted).and_then(Value::as_str).context("missing onSurfaceVariant")?;
        s.push_str(&format!("        muted = {},\n    )\n", kotlin_color(muted)));
    }
    s.push_str("}\n");
    Ok(s)
}
