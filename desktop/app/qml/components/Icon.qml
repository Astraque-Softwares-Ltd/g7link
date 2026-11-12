// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Shapes
import app.g7link

// A stroke icon from Icons (an SVG path on a 24×24 grid).
Item {
    id: icon
    property string path
    property color color: Theme.surfaceContent
    property real stroke: 1.8
    // G7's own filled glyph when it has one for this icon.
    readonly property string glyph: G7Glyphs.byPath[path] || ""
    readonly property int grid: glyph.length > 0 ? 16 : 24

    implicitWidth: 20
    implicitHeight: 20

    Shape {
        anchors.centerIn: parent
        width: icon.grid
        height: icon.grid
        scale: Math.min(icon.width, icon.height) / icon.grid
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: icon.glyph.length > 0 ? "transparent" : icon.color
            strokeWidth: icon.glyph.length > 0 ? -1 : icon.stroke
            fillColor: icon.glyph.length > 0 ? icon.color : "transparent"
            fillRule: ShapePath.OddEvenFill
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { path: icon.glyph.length > 0 ? icon.glyph : icon.path }
        }
    }
}
