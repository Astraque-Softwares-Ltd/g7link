// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A row's hover or selected fill: it fades in and out on the quick timing
// instead of snapping, and draws nothing while it's off.
Rectangle {
    id: fill
    property bool on: false
    opacity: on ? 1 : 0
    visible: opacity > 0
    Behavior on opacity {
        NumberAnimation {
            duration: fill.on ? Theme.quickIn : Theme.quickOut
            easing.type: Easing.BezierSpline
            easing.bezierCurve: Theme.easeMove
        }
    }
}
