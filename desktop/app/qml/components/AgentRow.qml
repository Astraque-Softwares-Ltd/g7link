// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A thread or a session in the Agents list: a state dot, the title and how
// long ago, then the agent, the folder and what it's doing; a running one
// adds "Now:" and the time it has been working. An unread thread's title is
// bolder; a fork's line starts "Fork of …", a brought-in one's "Brought in".
// On hover a thread offers Pin, Archive and its menu; Ctrl+click picks
// several; F2 renames it in place.
FocusScope {
    id: row
    // A row of Agents.threads or Agents.sessions.
    property var item: ({})
    property bool session: false
    property bool selected: false
    // Date.now(), ticking while something runs.
    property real now: Date.now()
    // Several threads are being picked: a box shows whether this one is.
    property bool picking: false
    property bool picked: false
    // The title is being renamed in place.
    property bool renaming: false
    // The host keeps marks (pin, archive, the menu).
    property bool marks: false
    signal clicked
    // Ctrl+click, or the box: picked or not.
    signal pickToggled
    signal pinRequested
    signal archiveRequested
    // The thread's menu, at (x, y) in the window.
    signal menuRequested(real x, real y)
    signal renameRequested
    signal renamed(string title)
    signal renameCancelled

    readonly property string phase: item.state || ""
    readonly property bool running: phase === "working" || phase === "waiting"
    readonly property bool showsStep: (phase === "working" || (session && phase === "needs_you")) && (item.step || "").length > 0
    readonly property real since: (item.since_ms || 0) > 0 ? item.since_ms : (item.updated_ms || 0)

    // The state's colour and shape (plans/agent-colour.md §2.9): a spinner
    // ring working, a filled dot with a bang needing you, an x failed, a
    // check done, a hollow ring paused; a guess is a hollow amber ring.
    readonly property bool ring: session && item.guess === true && phase === "needs_you"
    readonly property bool unread: !session && item.unread === true
    readonly property bool pinned: !session && item.pinned === true
    readonly property bool archived: !session && item.archived === true
    readonly property real snoozedUntil: session ? 0 : (item.snoozedUntil || 0)
    readonly property bool showsActions: !session && marks && !picking && !renaming && (mouse.containsMouse || row.activeFocus)
    // What the line under the title starts with: where the thread came from.
    readonly property string origin: {
        if (session)
            return ""
        if (snoozedUntil > 0)
            return qsTr("Snoozed until %1").arg(snoozeTime(snoozedUntil))
        if (((item.forkOf || {}).title || "").length > 0)
            return qsTr("Fork of %1").arg(item.forkOf.title)
        return item.found === true ? qsTr("Brought in") : ""
    }
    // "18:00" today, else "Tomorrow 09:00"-like: the day and the time.
    function snoozeTime(ms) {
        const when = new Date(ms)
        const time = when.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
        return new Date(row.now).toDateString() === when.toDateString()
            ? time : when.toLocaleDateString(Qt.locale(), "ddd") + " " + time
    }

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

    onRenamingChanged: {
        if (renaming) {
            renameInput.text = row.item.title || ""
            renameInput.selectAll()
            renameInput.forceActiveFocus()
        }
    }

    implicitHeight: texts.implicitHeight + 16
    activeFocusOnTab: true
    Accessible.role: Accessible.ListItem
    Accessible.name: row.item.title || ""
    Accessible.description: [row.item.agent_name, row.item.folder, row.doing].filter((s) => (s || "").length > 0).join(", ")
    Accessible.onPressAction: row.clicked()
    Keys.onPressed: (event) => {
        if (row.renaming)
            return
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
            row.clicked()
            event.accepted = true
        } else if (event.key === Qt.Key_F2 && !row.session && row.marks) {
            row.renameRequested()
            event.accepted = true
        } else if (event.key === Qt.Key_Menu && !row.session && row.marks) {
            const p = row.mapToItem(null, row.width - 8, row.height / 2)
            row.menuRequested(p.x, p.y)
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
        on: row.selected || row.picked
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
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onClicked: (mouseEvent) => {
            if (row.renaming)
                return
            if (mouseEvent.button === Qt.RightButton) {
                if (!row.session && row.marks) {
                    const p = row.mapToItem(null, mouseEvent.x, mouseEvent.y)
                    row.menuRequested(p.x, p.y)
                }
            } else if (!row.session && row.marks && (row.picking || (mouseEvent.modifiers & Qt.ControlModifier))) {
                row.pickToggled()
            } else {
                row.clicked()
            }
        }
    }

    AgentStateMark {
        id: dotMark
        visible: !row.picking
        x: Theme.rowPadding - 2
        y: 8 + Math.round((titleText.lineHeight - height) / 2)
        phase: row.phase
        guess: row.ring
    }
    // The box while several are picked.
    Rectangle {
        visible: row.picking
        x: Theme.rowPadding - 3
        y: 8 + Math.round((titleText.lineHeight - height) / 2)
        width: 14
        height: 14
        radius: 3
        color: row.picked ? Theme.accent : "transparent"
        border.width: row.picked ? 0 : 1.2
        border.color: Theme.outline
        Accessible.role: Accessible.CheckBox
        Accessible.checked: row.picked
        Accessible.name: qsTr("Select %1").arg(row.item.title || "")
        Icon {
            visible: row.picked
            anchors.centerIn: parent
            width: 11
            height: 11
            path: Icons.check
            color: Theme.accentContent
        }
    }

    Column {
        id: texts
        anchors.left: dotMark.right
        anchors.leftMargin: 8
        anchors.right: parent.right
        anchors.rightMargin: Theme.rowPadding
        y: 8
        spacing: 1

        Item {
            width: parent.width
            height: titleText.height
            Txt {
                id: titleText
                visible: !row.renaming
                anchors.left: parent.left
                anchors.right: rowEnd.left
                anchors.rightMargin: 8
                text: row.item.title || ""
                weight: row.unread ? 650 : 500
                maximumLineCount: 1
            }
            // Rename in place: Enter keeps it, Esc leaves it as it was.
            Rectangle {
                visible: row.renaming
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: titleText.verticalCenter
                height: titleText.height + 4
                radius: 4
                color: Theme.surface
                border.width: 2
                border.color: Theme.accent
                TextInput {
                    id: renameInput
                    x: 4
                    width: parent.width - 8
                    anchors.verticalCenter: parent.verticalCenter
                    clip: true
                    maximumLength: 80
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.body.size
                    font.weight: row.unread ? 650 : 500
                    color: Theme.surfaceContent
                    selectionColor: Theme.selection
                    selectedTextColor: Theme.surfaceContent
                    selectByMouse: true
                    Accessible.name: qsTr("Rename")
                    Keys.onReturnPressed: row.renamed(renameInput.text)
                    Keys.onEnterPressed: row.renamed(renameInput.text)
                    Keys.onEscapePressed: row.renameCancelled()
                    onActiveFocusChanged: if (!activeFocus && row.renaming) row.renameCancelled()
                }
            }
            Row {
                id: rowEnd
                anchors.right: parent.right
                anchors.verticalCenter: titleText.verticalCenter
                spacing: 4
                visible: !row.renaming
                Icon {
                    visible: row.pinned && !row.showsActions
                    anchors.verticalCenter: parent.verticalCenter
                    width: 12
                    height: 12
                    path: Icons.pin
                    color: Theme.surfaceContentVariant
                }
                Txt {
                    id: ageText
                    visible: !row.showsActions
                    anchors.verticalCenter: parent.verticalCenter
                    text: row.ago(row.item.updated_ms || 0)
                    role: "bodySmall"
                    muted: true
                }
                IconButton {
                    visible: row.showsActions
                    width: 24
                    height: 24
                    iconSize: 14
                    iconPath: Icons.pin
                    label: row.pinned ? qsTr("Unpin") : qsTr("Pin")
                    checked: row.pinned
                    onClicked: row.pinRequested()
                }
                IconButton {
                    visible: row.showsActions
                    width: 24
                    height: 24
                    iconSize: 14
                    iconPath: Icons.tray
                    label: row.archived ? qsTr("Unarchive") : qsTr("Archive")
                    onClicked: row.archiveRequested()
                }
                IconButton {
                    id: moreButton
                    visible: row.showsActions
                    width: 24
                    height: 24
                    iconSize: 14
                    iconPath: Icons.more
                    label: qsTr("More")
                    onClicked: {
                        const p = moreButton.mapToItem(null, moreButton.width, moreButton.height)
                        row.menuRequested(p.x, p.y)
                    }
                }
            }
        }
        Txt {
            width: parent.width
            text: [row.origin, row.item.agent_name, row.item.folder, row.doing].filter((s) => (s || "").length > 0).join(" · ")
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
