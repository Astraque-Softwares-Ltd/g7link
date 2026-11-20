// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A line under a tool about its request: waiting for you, answered on a
// phone, allowed by a rule, expired or cancelled.
Item {
    id: mark
    required property int depth
    required property string markKind
    required property string by

    // A waiting request was clicked: the page brings its dock into view.
    signal waitingClicked

    readonly property bool waiting: markKind === "waiting" || markKind === "held"
    readonly property string line: {
        switch (markKind) {
        case "waiting":
        case "held": return qsTr("Waiting for you")
        case "allowed": return by.length > 0 ? qsTr("Allowed on %1").arg(by) : qsTr("Allowed")
        case "denied": return by.length > 0 ? qsTr("Denied on %1").arg(by) : qsTr("Denied")
        case "rule": return qsTr("Allowed by your rule for this thread")
        case "expired": return qsTr("Expired")
        case "cancelled": return qsTr("Cancelled")
        default: return ""
        }
    }
    readonly property string icon: {
        switch (markKind) {
        case "allowed": return Icons.check
        case "rule": return Icons.shield
        case "denied":
        case "cancelled": return Icons.close
        default: return Icons.history
        }
    }

    width: ListView.view ? ListView.view.width : 0
    height: 22

    Accessible.role: mark.waiting ? Accessible.Button : Accessible.StaticText
    Accessible.name: line

    Row {
        x: mark.depth * 18 + 40
        anchors.verticalCenter: parent.verticalCenter
        spacing: 6
        Icon {
            anchors.verticalCenter: parent.verticalCenter
            width: 13
            height: 13
            path: mark.icon
            color: mark.waiting ? Theme.tintAmber : Theme.surfaceContentVariant
        }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            text: mark.line
            role: "caption"
            weight: mark.waiting ? 600 : Theme.typography.caption.weight
            color: mark.waiting ? Theme.tintAmber : Theme.surfaceContentVariant
        }
    }
    MouseArea {
        anchors.fill: parent
        enabled: mark.waiting
        cursorShape: Qt.PointingHandCursor
        onClicked: mark.waitingClicked()
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: mark.depth > 0
        x: (mark.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
