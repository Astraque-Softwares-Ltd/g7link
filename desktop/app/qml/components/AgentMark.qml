// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Shapes
import app.g7link

// An agent's own logo (AgentMarks), just before its name: never tinted, in
// the official colours, sharp at any size. Nothing is drawn for an agent
// without one.
Item {
    id: mark
    // "claude" or "codex".
    property string agent
    readonly property var spec: AgentMarks.of(agent)

    implicitWidth: 16
    implicitHeight: 16
    visible: spec !== null
    Accessible.ignored: true

    Shape {
        readonly property real unit: mark.spec ? Math.min(mark.width, mark.height) / mark.spec.box : 1
        x: mark.spec ? mark.spec.x * unit : 0
        y: mark.spec ? mark.spec.y * unit : 0
        width: mark.spec ? mark.spec.box - mark.spec.x : 0
        height: width
        scale: unit
        transformOrigin: Item.TopLeft
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            fillColor: mark.spec ? mark.spec.color : "transparent"
            strokeColor: "transparent"
            strokeWidth: -1
            fillRule: ShapePath.WindingFill
            PathSvg { path: mark.spec ? mark.spec.path : "" }
        }
    }
}
