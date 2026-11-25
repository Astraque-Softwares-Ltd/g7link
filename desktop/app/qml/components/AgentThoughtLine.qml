// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// What the agent thought: its first line, or "Thought" when the agent
// doesn't share the text. Opens to the whole thought when there is one.
Item {
    id: thought
    required property string key
    required property int depth
    required property string text
    required property int count
    required property bool hasText
    required property bool isOpen

    readonly property string line: text.length > 0 ? text
        : (count > 1 ? qsTr("Thought %n times", "", count) : qsTr("Thought"))

    width: ListView.view ? ListView.view.width : 0
    height: Math.max(28, label.height + 8)

    Accessible.role: thought.hasText ? Accessible.Button : Accessible.StaticText
    Accessible.name: line
    Accessible.onPressAction: if (thought.hasText) AgentMotion.toggle(thought.key)

    Icon {
        visible: thought.hasText
        x: thought.depth * 18
        y: 7
        width: 14
        height: 14
        path: Icons.chevronRight
        rotation: thought.isOpen ? 90 : 0
        Behavior on rotation {
            enabled: AgentMotion.gliding
            NumberAnimation { duration: AgentMotion.turnMs; easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeMove }
        }
        color: Theme.surfaceContentVariant
    }
    Icon {
        id: bulb
        x: thought.depth * 18 + 20
        y: 6
        width: 16
        height: 16
        path: Icons.lightbulb
        color: Theme.surfaceContentVariant
    }
    Txt {
        id: label
        anchors.left: bulb.right
        anchors.leftMargin: 8
        anchors.right: parent.right
        y: 4
        text: thought.line
        role: "bodySmall"
        muted: true
        maximumLineCount: 2
        wrapMode: Text.WordWrap
    }
    MouseArea {
        anchors.fill: parent
        enabled: thought.hasText
        cursorShape: Qt.PointingHandCursor
        onClicked: AgentMotion.toggle(thought.key)
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: thought.depth > 0
        x: (thought.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
