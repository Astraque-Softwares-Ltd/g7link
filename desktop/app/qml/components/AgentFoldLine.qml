// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// "Worked for 2m 14s": everything a finished turn did, folded until opened.
Item {
    id: fold
    required property string key
    required property int depth
    required property real workedMs
    required property int hidden
    required property bool isOpen

    // "12s", "2m 14s", "1h 03m".
    function duration(ms) {
        const total = Math.max(0, Math.floor(ms / 1000))
        if (total < 60)
            return qsTr("%1s").arg(total)
        const minutes = Math.floor(total / 60)
        if (minutes < 60)
            return qsTr("%1m %2s").arg(minutes).arg(String(total % 60).padStart(2, "0"))
        return qsTr("%1h %2m").arg(Math.floor(minutes / 60)).arg(String(minutes % 60).padStart(2, "0"))
    }

    width: ListView.view ? ListView.view.width : 0
    height: 30

    Accessible.role: Accessible.Button
    Accessible.name: label.text
    Accessible.onPressAction: AgentMotion.toggle(fold.key)

    Row {
        id: line
        x: fold.depth * 18
        anchors.verticalCenter: parent.verticalCenter
        spacing: 6
        Icon {
            anchors.verticalCenter: parent.verticalCenter
            width: 14
            height: 14
            // Turns as the fold opens, rather than swapping.
            path: Icons.chevronRight
            rotation: fold.isOpen ? 90 : 0
            Behavior on rotation {
                enabled: AgentMotion.gliding
                NumberAnimation { duration: AgentMotion.turnMs; easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeMove }
            }
            color: Theme.surfaceContentVariant
        }
        Txt {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Worked for %1").arg(fold.duration(fold.workedMs))
            role: "bodySmall"
            muted: true
        }
    }
    Rectangle {
        anchors.left: line.right
        anchors.leftMargin: 10
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        height: 1
        color: Theme.line
    }
    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: AgentMotion.toggle(fold.key)
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: fold.depth > 0
        x: (fold.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
