// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// What can be done from a message, under it on hover: Copy, and the host's
// verdicts for its turn. On your message: Edit from here, Rewind to here,
// Fork from here; on the final answer: Fork from here. One that can't be
// done shows disabled and says why while the pointer is on it. The bridge
// hides them all while the thread works or waits.
Row {
    id: actions
    // The item whose whole text Copy takes.
    property string itemId
    // The words of the row's actions: "" (hidden), "on", "latest_only",
    // "no_restore_point" or "not_git".
    property string fork
    property string rewind
    property string edit
    property string agentName
    // Also offers Edit from here and Rewind to here (your message).
    property bool mine: false

    signal editRequested
    signal rewindRequested
    signal forkRequested

    readonly property bool any: fork.length > 0 || rewind.length > 0 || edit.length > 0
    property string why: ""

    function reason(word) {
        switch (word) {
        case "latest_only": return qsTr("%1 can only be forked from its latest message.").arg(actions.agentName)
        case "no_restore_point": return qsTr("No restore point for this turn.")
        case "not_git": return qsTr("This isn't a git repository, so G7 can't undo file changes here.")
        default: return ""
        }
    }

    spacing: 8
    layoutDirection: mine ? Qt.RightToLeft : Qt.LeftToRight

    component Act: Item {
        id: act
        property string label
        // "" hides it; "on" offers it; any other word disables it with its reason.
        property string word: "on"
        property bool done: false
        signal triggered
        readonly property bool on: word === "on"
        visible: word.length > 0
        width: actText.implicitWidth + 16
        height: 24
        activeFocusOnTab: on
        Accessible.role: Accessible.Button
        Accessible.name: label
        Accessible.description: on ? "" : actions.reason(word)
        Accessible.onPressAction: if (act.on) act.triggered()
        Keys.onPressed: (event) => {
            if (act.on && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
                act.triggered()
                event.accepted = true
            }
        }
        Rectangle {
            anchors.fill: parent
            radius: Theme.radiusSm
            color: Theme.surfaceContentVariant
            opacity: act.on && actHover.hovered ? 0.08 : 0
        }
        Rectangle {
            anchors.fill: parent
            radius: Theme.radiusSm
            color: "transparent"
            visible: Theme.focusVisible(act)
            border.width: 2
            border.color: Theme.focusRing
        }
        Txt {
            id: actText
            anchors.centerIn: parent
            text: act.label
            role: "bodySmall"
            weight: 500
            muted: !act.on
        }
        HoverHandler {
            id: actHover
            onHoveredChanged: actions.why = hovered && !act.on ? actions.reason(act.word) : ""
        }
        MouseArea {
            anchors.fill: parent
            enabled: act.on
            cursorShape: Qt.PointingHandCursor
            onClicked: act.triggered()
        }
    }

    Act {
        id: copyAct
        label: done ? qsTr("Copied") : qsTr("Copy")
        onTriggered: {
            if (Agents.copyText("i:" + actions.itemId)) {
                copyAct.done = true
                copiedTimer.restart()
            }
        }
        Timer {
            id: copiedTimer
            interval: 1500
            onTriggered: copyAct.done = false
        }
    }
    Act {
        label: qsTr("Edit from here")
        word: actions.mine ? actions.edit : ""
        onTriggered: actions.editRequested()
    }
    Act {
        label: qsTr("Rewind to here")
        word: actions.mine ? actions.rewind : ""
        onTriggered: actions.rewindRequested()
    }
    Act {
        label: qsTr("Fork from here")
        word: actions.fork
        onTriggered: actions.forkRequested()
    }
    Txt {
        visible: actions.why.length > 0
        anchors.verticalCenter: parent.verticalCenter
        text: actions.why
        role: "caption"
        muted: true
    }
}
