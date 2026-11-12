#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Writes G7 Link's color-only themes from G7 Linux's theme files.

    python3 scripts/gen-palettes.py <g7linux checkout>/extra/themes

Each family keeps the launcher's core colors and accents as they are; both
apps map them onto the G7 theme's roles at runtime (desktop/app/qml/Theme.qml,
android .../ui/theme/Theme.kt), so the layout never changes with the palette.
The output is committed; nothing runs this during a build.
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
QML_OUT = ROOT / "desktop/app/qml/Palettes.qml"
KOTLIN_OUT = ROOT / "android/app/src/main/kotlin/app/g7link/android/ui/theme/Palettes.kt"

# (stored id, shown name, light theme file, dark theme file); no light file: dark only.
FAMILIES = [
    ("catppuccin", "Catppuccin", "catppuccin-latte", "catppuccin-mocha"),
    ("gruvbox", "Gruvbox", "gruvbox-light", "gruvbox-dark"),
    ("rose-pine", "Rosé Pine", "rose-pine-dawn", "rose-pine"),
    ("solarized", "Solarized", "solarized-light", "solarized-dark"),
    ("nord", "Nord", "nord-light", "nord"),
    ("tokyo-night", "Tokyo Night", None, "tokyo-night"),
    ("kanagawa", "Kanagawa", None, "kanagawa"),
    ("dracula", "Dracula", None, "dracula"),
    ("ayu", "Ayu", None, "ayu-dark"),
]

CORE = ["background", "foreground", "border", "accent"]
ACCENTS = ["blue", "green", "magenta", "orange", "purple", "red", "yellow", "cyan"]
KEYS = CORE + ACCENTS

# The launcher's built-in themes, which every theme file falls back to.
BUILTIN = {
    "g7linux-dark": {
        "background": "#18181b", "foreground": "#f2f2f3", "border": "#303035", "accent": "#b292d0",
        "blue": "#7aa2f7", "green": "#6cc08b", "magenta": "#b292d0", "orange": "#e8a05c",
        "purple": "#b292d0", "red": "#e0785f", "yellow": "#d9bf72", "cyan": "#6ab7c2",
    },
    "g7linux-light": {
        "background": "#fafafb", "foreground": "#1c1c1f", "border": "#dcdce1", "accent": "#7d5ca9",
        "blue": "#3557c2", "green": "#2d7a4d", "magenta": "#7d5ca9", "orange": "#b8661f",
        "purple": "#7d5ca9", "red": "#b0432b", "yellow": "#8f6f1f", "cyan": "#2f7480",
    },
}


def hex_color(value: str) -> str:
    v = value.strip().lstrip("#")
    if len(v) == 3:
        v = "".join(c * 2 for c in v)
    if len(v) == 8:
        v = v[2:]
    if len(v) != 6 or any(c not in "0123456789abcdefABCDEF" for c in v):
        raise ValueError(f"{value!r} is not a color")
    return "#" + v.lower()


def raw_colors(doc: dict) -> dict:
    colors = doc.get("colors", {})
    out = {}
    for key in CORE:
        if key in colors.get("core", {}):
            out[key] = colors["core"][key]
    for key in ACCENTS:
        if key in colors.get("accents", {}):
            out[key] = colors["accents"][key]
    return out


def resolve(themes_dir: Path, theme_id: str, seen=()) -> dict:
    if theme_id in BUILTIN:
        return dict(BUILTIN[theme_id])
    if theme_id in seen:
        raise ValueError(f"{theme_id}: inherits itself")
    doc = tomllib.loads((themes_dir / f"{theme_id}.toml").read_text())
    meta = doc["meta"]
    parent_id = meta.get("inherits") or ("g7linux-light" if meta.get("variant") == "light" else "g7linux-dark")
    colors = resolve(themes_dir, parent_id, seen + (theme_id,))
    own = raw_colors(doc)
    # References name another core color or accent ("colors.accents.blue").
    for key, value in own.items():
        if isinstance(value, dict):
            raise ValueError(f"{theme_id}.{key}: only plain colors and references are supported here")
        if value.startswith("colors."):
            target = value.split(".")[-1]
            value = own.get(target, colors.get(target))
        colors[key] = hex_color(value)
    return colors


def qml_variant(colors: dict | None) -> str:
    if colors is None:
        return "null"
    return "{ " + ", ".join(f'"{k}": "{colors[k]}"' for k in KEYS) + " }"


def render_qml(families: list) -> str:
    lines = [
        "// SPDX-License-Identifier: GPL-3.0-or-later",
        "// Generated from G7 Linux's theme files by scripts/gen-palettes.py. Do not edit.",
        "pragma Singleton",
        "import QtQuick",
        "",
        "QtObject {",
        "    // Color-only themes: each family's light and dark colors (light is null",
        "    // when it's dark only), mapped onto the G7 theme's roles by Theme.",
        "    readonly property var families: [",
    ]
    for fid, name, light, dark in families:
        lines.append("        {")
        lines.append(f'            "id": "{fid}",')
        lines.append(f'            "name": "{name}",')
        lines.append(f'            "light": {qml_variant(light)},')
        lines.append(f'            "dark": {qml_variant(dark)}')
        lines.append("        },")
    lines[-1] = "        }"
    lines += ["    ]", "}", ""]
    return "\n".join(lines)


def kotlin_color(hex_value: str) -> str:
    return f"Color(0xFF{hex_value[1:].upper()})"


def kotlin_variant(colors: dict | None, indent: str) -> str:
    if colors is None:
        return "null"
    lines = ["LauncherColors("]
    lines += [f"{indent}    {k} = {kotlin_color(colors[k])}," for k in KEYS]
    lines.append(f"{indent})")
    return "\n".join(lines)


def render_kotlin(families: list) -> str:
    lines = [
        "// SPDX-License-Identifier: GPL-3.0-or-later",
        "// Generated from G7 Linux's theme files by scripts/gen-palettes.py. Do not edit.",
        "package app.g7link.android.ui.theme",
        "",
        "import androidx.compose.runtime.Immutable",
        "import androidx.compose.ui.graphics.Color",
        "",
        "/** One variant of a G7 Linux theme: its core colors and accents. */",
        "@Immutable",
        "data class LauncherColors(",
    ]
    lines += [f"    val {k}: Color," for k in KEYS]
    lines += [
        ")",
        "",
        "/** A color-only theme: a G7 Linux theme family, light and dark ([light] null when it's dark only). */",
        "@Immutable",
        "data class PaletteFamily(",
        "    val id: String,",
        "    val name: String,",
        "    val light: LauncherColors?,",
        "    val dark: LauncherColors,",
        ")",
        "",
        "object Palettes {",
        "    val families: List<PaletteFamily> = listOf(",
    ]
    for fid, name, light, dark in families:
        lines += [
            "        PaletteFamily(",
            f'            id = "{fid}",',
            f'            name = "{name}",',
            f"            light = {kotlin_variant(light, '            ')},",
            f"            dark = {kotlin_variant(dark, '            ')},",
            "        ),",
        ]
    lines += ["    )", "}", ""]
    return "\n".join(lines)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    themes_dir = Path(sys.argv[1])
    families = []
    for fid, name, light_id, dark_id in FAMILIES:
        light = resolve(themes_dir, light_id) if light_id else None
        dark = resolve(themes_dir, dark_id)
        families.append((fid, name, light, dark))
    QML_OUT.write_text(render_qml(families))
    KOTLIN_OUT.write_text(render_kotlin(families))
    print(f"wrote {QML_OUT.relative_to(ROOT)} and {KOTLIN_OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
