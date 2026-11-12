// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Text in one of the design's type roles: displayLarge, displaySmall,
// headline, title, body, bodySmall, label, caption, code.
Text {
    id: txt
    property string role: "body"
    // Secondary text uses the variant color.
    property bool muted: false

    readonly property var spec: Theme.typography[role] || Theme.typography.body

    // Pixel size; set this (not font.pixelSize) to resize, so the role's
    // tracking scales with it.
    property real size: Math.round(spec.size)
    // Set this (not font.weight) to change the weight: Geist Mono is a
    // variable font, and its weight axis is set to match, so in-between weights such
    // as 650 render exactly instead of snapping to a named style.
    property int weight: spec.weight

    color: muted ? Theme.surfaceContentVariant : Theme.surfaceContent
    font.family: role === "code" ? Theme.fontMono : Theme.fontUi
    font.pixelSize: Math.max(1, Math.round(size))
    font.weight: weight
    font.variableAxes: ({ "wght": weight })
    font.letterSpacing: spec.tracking * size
    // Token line heights are CSS-style (a multiple of the font size), not
    // a multiple of the font's own line spacing.
    lineHeight: Math.round(Math.max(1, Math.round(size)) * spec.lineHeight)
    lineHeightMode: Text.FixedHeight
    verticalAlignment: Text.AlignVCenter
    textFormat: Text.PlainText
    elide: Text.ElideRight
}
