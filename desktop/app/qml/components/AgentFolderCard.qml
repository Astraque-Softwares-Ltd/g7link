// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// One of your agent folders in the new thread's Where: its name, how long
// ago a thread was started there and with which agent, and the branch and
// title of its newest thread ("Not used yet" before any). The chosen one
// has the accent's border; the keyboard's current one is lit.
FocusScope {
    id: card
    // A folder of Agents.setup: { id, label, shownPath, usedMs, repo, last: { agent, agentName, model, effort, title, branch } }.
    property var folder: ({})
    // "2 h": how long since it was used ("" before any thread).
    property string when
    property bool selected: false
    property bool current: false
    signal picked

    readonly property var last: folder.last || null
    readonly property string detail: last
        ? [last.branch || "", last.title || ""].filter((s) => s.length > 0).join(" · ")
        : qsTr("Not used yet")
    readonly property string runWith: last
        ? [last.agentName, last.model, last.effort].filter((s) => (s || "").length > 0).join(" · ") : ""

    implicitHeight: 56
    activeFocusOnTab: true
    Accessible.role: Accessible.Button
    Accessible.name: [folder.label || "", when, detail, runWith].filter((s) => s.length > 0).join(", ")
    Accessible.selected: selected
    Accessible.onPressAction: card.picked()
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
            card.picked()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: card.selected ? Theme.activeFill : (mouse.containsMouse || card.current ? Theme.raised : "transparent")
        border.width: 1
        border.color: card.selected ? Theme.accent : Theme.line
        Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
    }
    Rectangle {
        anchors.fill: parent
        anchors.margins: -3
        radius: Theme.rowRadius + 3
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: Theme.focusVisible(card)
    }

    Icon {
        id: folderIcon
        x: 10
        y: 10
        width: 16
        height: 16
        path: Icons.folder
        color: Theme.tintAmber
    }
    Row {
        id: side
        anchors.right: parent.right
        anchors.rightMargin: 10
        y: 10
        height: 16
        spacing: 5
        AgentMark {
            visible: card.last !== null
            anchors.verticalCenter: parent.verticalCenter
            width: 12
            height: 12
            agent: card.last ? card.last.agent : ""
        }
        Txt {
            visible: text.length > 0
            anchors.verticalCenter: parent.verticalCenter
            text: card.when
            role: "caption"
            muted: true
        }
    }
    Txt {
        anchors.left: folderIcon.right
        anchors.leftMargin: 8
        anchors.right: side.left
        anchors.rightMargin: 8
        anchors.verticalCenter: folderIcon.verticalCenter
        text: card.folder.label || ""
        role: "body"
        weight: 600
        maximumLineCount: 1
    }
    Row {
        anchors.left: folderIcon.right
        anchors.leftMargin: 8
        anchors.right: parent.right
        anchors.rightMargin: 10
        y: 32
        height: 16
        spacing: 4
        Icon {
            id: branchIcon
            visible: card.last !== null && (card.last.branch || "").length > 0
            anchors.verticalCenter: parent.verticalCenter
            width: 12
            height: 12
            path: AgentGitGlyphs.branch
            color: Theme.surfaceContentVariant
        }
        Txt {
            width: parent.width - (branchIcon.visible ? branchIcon.width + parent.spacing : 0)
            anchors.verticalCenter: parent.verticalCenter
            text: card.detail
            role: "bodySmall"
            muted: true
            maximumLineCount: 1
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: card.picked()
    }
}
