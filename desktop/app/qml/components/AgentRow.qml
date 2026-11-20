// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A thread or a session in the Agents list: a state dot, the title and how
// long ago, then the agent, the folder and what it's doing; a running one
// adds "Now:" and the time it has been working.
FocusScope {
    id: row
    // A row of Agents.threads or Agents.sessions.
    property var item: ({})
    property bool session: false
    property bool selected: false
    // Date.now(), ticking while something runs.
    property real now: Date.now()
    signal clicked

    readonly property string phase: item.state || ""
    readonly property bool running: phase === "working" || phase === "waiting"
    readonly property bool showsStep: (phase === "working" || (session && phase === "needs_you")) && (item.step || "").length > 0
    readonly property real since: (item.since_ms || 0) > 0 ? item.since_ms : (item.updated_ms || 0)

    // The state's colour; a guess is drawn as a ring.
    readonly property color dot: {
        switch (phase) {
        case "waiting":
        case "needs_you": return Theme.tintAmber
        case "working": return Theme.tintBlue
        case "done":
        case "idle": return Theme.tintTeal
        case "failed": return Theme.error
        default: return Theme.outline
        }
    }
    readonly property bool ring: session && item.guess === true && phase === "needs_you"

    // What it's doing, after the agent and the folder.
    readonly property string doing: {
        if (session) {
            switch (phase) {
            case "needs_you":
                if (item.guess)
                    return qsTr("may need you")
                if (item.source === "terminal")
                    return qsTr("waiting in the terminal")
                if (item.source === "desktop_app")
                    return item.agent === "codex" ? qsTr("waiting in the Codex app") : qsTr("waiting in the Claude app")
                return qsTr("waiting in another app")
            case "working": return ""
            case "idle": return qsTr("idle")
            default: return qsTr("ended")
            }
        }
        switch (phase) {
        case "waiting":
            // A question the agent asks, and no request beside it.
            if ((item.asking || 0) > 0 && (item.pending || 0) === 0)
                return item.asking > 1 ? qsTr("asks you %n questions", "", item.asking) : qsTr("asks you a question")
            return qsTr("needs you")
        case "working": return ""
        case "done": return item.plan_ready === true ? qsTr("plan ready") : qsTr("done")
        case "failed": return item.error_code === "usage_limit" ? qsTr("hit its usage limit") : qsTr("failed")
        case "interrupted": return qsTr("stopped")
        default: return ""
        }
    }

    // "2 min", "1 h", "3 d".
    function ago(ms) {
        const minutes = Math.max(0, Math.floor((row.now - ms) / 60000))
        if (minutes < 1)
            return qsTr("now")
        if (minutes < 60)
            return qsTr("%1 min").arg(minutes)
        const hours = Math.floor(minutes / 60)
        return hours < 24 ? qsTr("%1 h").arg(hours) : qsTr("%1 d").arg(Math.floor(hours / 24))
    }
    // "4:12", "1:03:20".
    function elapsed(ms) {
        const total = Math.max(0, Math.floor((row.now - ms) / 1000))
        const h = Math.floor(total / 3600)
        const m = Math.floor((total % 3600) / 60)
        const s = String(total % 60).padStart(2, "0")
        return h > 0 ? h + ":" + String(m).padStart(2, "0") + ":" + s : m + ":" + s
    }

    implicitHeight: texts.implicitHeight + 16
    activeFocusOnTab: true
    Accessible.role: Accessible.ListItem
    Accessible.name: row.item.title || ""
    Accessible.description: [row.item.agent_name, row.item.folder, row.doing].filter((s) => (s || "").length > 0).join(", ")
    Accessible.onPressAction: row.clicked()
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
            row.clicked()
            event.accepted = true
        }
    }

    StateFill {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: Theme.raised
        on: !row.selected && mouse.containsMouse
    }
    StateFill {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: Theme.selection
        on: row.selected
    }
    Rectangle {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: "transparent"
        visible: Theme.focusVisible(row)
        border.width: 2
        border.color: Theme.focusRing
    }
    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: row.clicked()
    }

    Rectangle {
        id: dotMark
        x: Theme.rowPadding
        y: 8 + Math.round((titleText.lineHeight - height) / 2)
        width: 8
        height: 8
        radius: 4
        color: row.ring ? "transparent" : row.dot
        border.width: row.ring ? 1.5 : 0
        border.color: row.dot
    }

    Column {
        id: texts
        anchors.left: dotMark.right
        anchors.leftMargin: 10
        anchors.right: parent.right
        anchors.rightMargin: Theme.rowPadding
        y: 8
        spacing: 1

        Item {
            width: parent.width
            height: titleText.height
            Txt {
                id: titleText
                anchors.left: parent.left
                anchors.right: ageText.left
                anchors.rightMargin: 8
                text: row.item.title || ""
                weight: 500
                maximumLineCount: 1
            }
            Txt {
                id: ageText
                anchors.right: parent.right
                anchors.verticalCenter: titleText.verticalCenter
                text: row.ago(row.item.updated_ms || 0)
                role: "bodySmall"
                muted: true
            }
        }
        Txt {
            width: parent.width
            text: [row.item.agent_name, row.item.folder, row.doing].filter((s) => (s || "").length > 0).join(" · ")
            role: "bodySmall"
            muted: true
            maximumLineCount: 1
        }
        Txt {
            visible: row.showsStep
            width: parent.width
            text: qsTr("Now: %1").arg(row.item.step || "") + " · " + row.elapsed(row.since)
            role: "bodySmall"
            maximumLineCount: 1
        }
    }
}
