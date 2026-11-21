// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQml.Models
import app.g7link

// An agent thread as it reads: what you wrote, what it thought and did (a
// line a step, runs of steps as one line, a finished turn folded behind
// "Worked for …"), its answer as Markdown, and the live line while it works.
// It follows the newest until you scroll up; then "Jump to latest" brings
// you back. Rows come from AgentTimeline, computed off the GUI thread.
ListView {
    id: timeline
    // Where it runs, beside an opened command.
    property string folder
    // Date.now(), ticking.
    property real now: Date.now()
    // The page is on screen: the live line's band may move.
    property bool animate: true
    // Keeps the newest in view until the user scrolls up.
    property bool follow: true
    // A turn runs or waits: a plan can't be implemented or changed now.
    property bool running: false
    // "Claude" or "Codex", for the reason an action can't be done.
    property string agentName
    // The message a search hit opened the thread at, lit up for a moment.
    property string highlightItem: ""

    // A link was clicked: the page shows where it goes, then opens it.
    signal linkActivated(string key, int index)
    // A "Waiting for you" line was clicked: the page brings the dock into view.
    signal waitingClicked
    // A plan's Implement ("supervised" or "auto_edit").
    signal implementPlan(string itemId, string mode)
    // A plan's mode menu, under its Implement (in the window's coordinates).
    signal implementMenuRequested(string itemId, real x, real y)
    // "Change the plan": the page puts the keyboard in the composer.
    signal changePlan
    // A message's actions, for its turn.
    signal editFromHere(int turn)
    signal rewindTo(int turn)
    signal forkFrom(int turn)
    // A recorded turn's Changes: the page opens its diff, at `path` ("" for its first file).
    signal openChanges(int turn, string path)
    // Undo on the newest rewind's notice.
    signal undoRewind
    // The first message on screen once the user stops scrolling ("" while it
    // follows the newest): where a phone opens it when it's handed over.
    signal readingAt(string itemId)

    function jumpToLatest() {
        follow = true
        positionViewAtEnd()
    }
    // The item of the first row at the top of the view ("" for none).
    function topItem() {
        for (let y = 2; y < height; y += 12) {
            const row = indexAt(width / 2, contentY + y)
            if (row >= 0)
                return AgentTimeline.itemIdAt(row)
        }
        return ""
    }

    model: AgentTimeline
    clip: true
    spacing: 0
    boundsBehavior: Flickable.StopAtBounds
    // Rows past the view are laid out ahead, so heights settle before they show.
    cacheBuffer: 1200
    onMovementEnded: {
        follow = atYEnd
        if (!follow)
            readingAt(topItem())
    }
    onFollowChanged: if (follow) readingAt("")
    onAtYEndChanged: if (atYEnd) follow = true
    onContentHeightChanged: if (follow) Qt.callLater(() => timeline.positionViewAtEnd())
    onCountChanged: if (follow) Qt.callLater(() => timeline.positionViewAtEnd())

    // Scrolling up to read stops following the end.
    WheelHandler {
        target: null
        blocking: false
        onWheel: (event) => {
            if (event.angleDelta.y > 0 || event.pixelDelta.y > 0)
                timeline.follow = false
        }
    }

    // A circled question mark (Tabler's help-circle, MIT, as Icons.qml credits).
    readonly property string questionIcon: "M3 12a9 9 0 1 0 18 0 9 9 0 1 0-18 0M12 17v.01M12 13.5a1.5 1.5 0 0 1 1-1.5 2.6 2.6 0 1 0-3-4"

    // What you wrote: on the right, in a bubble; a denial's words say so, and
    // so does a message sent while the agent worked.
    component UserRow: Item {
        id: user
        required property string text
        required property string from
        required property bool denyOf
        required property int depth
        required property string via
        required property int images
        required property string itemId
        required property int turn
        required property string actFork
        required property string actRewind
        required property string actEdit
        readonly property bool lit: timeline.highlightItem.length > 0 && timeline.highlightItem === itemId
        width: ListView.view ? ListView.view.width : 0
        height: column.height + 14
        HoverHandler {
            id: userHover
        }
        Column {
            id: column
            anchors.right: parent.right
            y: 7
            width: Math.min(parent.width * 0.78, Math.max(bubbleText.implicitWidth + 24, said.implicitWidth))
            spacing: 3
            Txt {
                id: said
                visible: user.denyOf || user.via === "steer"
                anchors.right: parent.right
                text: user.denyOf
                    ? (user.from.length > 0 ? qsTr("%1 denied it and said").arg(user.from) : qsTr("You denied it and said"))
                    : qsTr("Sent while it worked")
                role: "caption"
                muted: true
            }
            Rectangle {
                anchors.right: parent.right
                width: Math.min(parent.width, bubbleText.implicitWidth + 24)
                height: bubbleText.height + 16
                radius: Theme.radiusMd
                color: user.lit ? Theme.selection : Theme.raised
                border.width: user.lit ? 1.5 : 0
                border.color: Theme.accent
                Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
                TextEdit {
                    id: bubbleText
                    x: 12
                    y: 8
                    width: Math.min(column.width - 24, implicitWidth)
                    text: user.text
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.Wrap
                    textFormat: TextEdit.PlainText
                    color: Theme.surfaceContent
                    selectionColor: Theme.selection
                    selectedTextColor: Theme.surfaceContent
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.body.size
                }
            }
            Row {
                visible: user.images > 0
                anchors.right: parent.right
                spacing: 4
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 13
                    height: 13
                    path: Icons.photo
                    color: Theme.surfaceContentVariant
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("%n image(s)", "", user.images)
                    role: "caption"
                    muted: true
                }
            }
            Txt {
                visible: user.from.length > 0 && !user.denyOf
                anchors.right: parent.right
                text: qsTr("From %1").arg(user.from)
                role: "caption"
                muted: true
            }
            // Copy, Edit from here, Rewind to here, Fork from here: on hover.
            Item {
                id: userActs
                visible: acts.any
                anchors.right: parent.right
                width: Math.min(column.width, acts.implicitWidth)
                height: visible ? 26 : 0
                AgentRowActions {
                    id: acts
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    mine: true
                    itemId: user.itemId
                    fork: user.actFork
                    rewind: user.actRewind
                    edit: user.actEdit
                    agentName: timeline.agentName
                    opacity: userHover.hovered || activeFocus ? 1 : 0
                    Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
                    onEditRequested: timeline.editFromHere(user.turn)
                    onRewindRequested: timeline.rewindTo(user.turn)
                    onForkRequested: timeline.forkFrom(user.turn)
                }
            }
        }
    }

    // The agent asked: one line, opened to show the answers when it has them.
    component QuestionRow: Item {
        id: asked
        required property string key
        required property int depth
        required property string status
        required property int count
        required property string by
        required property bool hasText
        required property bool isOpen
        readonly property string line: {
            switch (status) {
            case "answered": return by.length > 0 ? qsTr("Answered on %1").arg(by) : qsTr("You answered")
            case "dismissed":
            case "expired": return qsTr("Not answered")
            default: return count > 1 ? qsTr("Asked you %n questions", "", count) : qsTr("Asked you a question")
            }
        }
        width: ListView.view ? ListView.view.width : 0
        height: 30
        Accessible.role: asked.hasText ? Accessible.Button : Accessible.StaticText
        Accessible.name: asked.line
        Accessible.onPressAction: if (asked.hasText) Agents.toggle(asked.key)
        Rectangle {
            visible: asked.depth > 0
            x: (asked.depth - 1) * 18 + 8
            width: 1
            height: parent.height
            color: Theme.line
        }
        Row {
            x: asked.depth * 18 + 20
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 16
                height: 16
                path: timeline.questionIcon
                color: asked.status === "asked" ? AgentPalette.needsYou : Theme.surfaceContentVariant
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: asked.line
                role: "bodySmall"
                muted: asked.status !== "asked"
            }
            Icon {
                visible: asked.hasText
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: asked.isOpen ? Icons.chevronUp : Icons.chevronDown
                color: Theme.surfaceContentVariant
            }
        }
        MouseArea {
            anchors.fill: parent
            enabled: asked.hasText
            cursorShape: Qt.PointingHandCursor
            onClicked: Agents.toggle(asked.key)
        }
    }

    // A note from G7 or the host (a stop, a reply that wasn't sent).
    component NoticeRow: Item {
        id: noticeRow
        required property string text
        required property int depth
        // "rewound", "edit_from_here", "files_restored", "forked" or "".
        required property string notice
        // The newest rewind's notice while it can be undone.
        required property bool undo
        readonly property bool rewound: notice === "rewound" || notice === "edit_from_here"
        width: ListView.view ? ListView.view.width : 0
        height: Math.max(noticeText.height + 12, undoButton.visible ? 32 : 0)
        Icon {
            x: noticeRow.depth * 18 + 20
            y: 7
            width: 16
            height: 16
            path: noticeRow.rewound ? Icons.history : (noticeRow.notice === "files_restored" ? Icons.check : Icons.info)
            color: noticeRow.rewound ? Theme.surfaceContent : Theme.surfaceContentVariant
        }
        Button {
            id: undoButton
            visible: noticeRow.undo
            x: Math.min(noticeRow.depth * 18 + 44 + noticeText.implicitWidth + 12, parent.width - width)
            y: 2
            variant: "tonal"
            size: "sm"
            text: qsTr("Undo")
            onClicked: timeline.undoRewind()
        }
        Rectangle {
            visible: noticeRow.depth > 0
            x: (noticeRow.depth - 1) * 18 + 8
            width: 1
            height: parent.height
            color: Theme.line
        }
        Txt {
            id: noticeText
            x: noticeRow.depth * 18 + 44
            y: noticeRow.undo ? 8 : 6
            width: Math.min(implicitWidth, parent.width - x - (undoButton.visible ? undoButton.width + 12 : 0))
            text: noticeRow.notice === "rewound" || noticeRow.notice === "edit_from_here" ? qsTr("Rewound to here") : noticeRow.text
            role: "bodySmall"
            muted: !noticeRow.rewound
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }

    // In place of, or under, a turn's Changes: still recording, not recorded
    // (and why), no longer kept, or files too large to record.
    component NoteRow: Item {
        id: noteRow
        required property int depth
        // "recording", "missing", "expired" or "unrecorded".
        required property string note
        required property string why
        required property int count
        readonly property string line: {
            switch (note) {
            case "recording": return qsTr("Recording changes…")
            case "expired": return qsTr("This restore point is no longer kept.")
            case "unrecorded": return qsTr("%n files weren't recorded (too large).", "", count)
            default: {
                const reason = why === "busy" ? qsTr("Git was busy.")
                    : why === "slow" ? qsTr("It took too long.")
                    : why === "large" ? qsTr("Too many large new files.")
                    : why === "refused" ? qsTr("Git refused.")
                    : why === "off" ? qsTr("Restore points are off for this folder.") : ""
                const missing = qsTr("Changes weren't recorded for this turn.")
                return reason.length > 0 ? missing + " " + reason : missing
            }
            }
        }
        width: ListView.view ? ListView.view.width : 0
        height: noteText.height + 12
        Spinner {
            visible: noteRow.note === "recording"
            x: noteRow.depth * 18 + 21
            y: 7
            width: 14
            height: 14
        }
        Icon {
            visible: noteRow.note !== "recording"
            x: noteRow.depth * 18 + 20
            y: 7
            width: 16
            height: 16
            path: Icons.edit
            color: Theme.surfaceContentVariant
        }
        Txt {
            id: noteText
            x: noteRow.depth * 18 + 44
            y: 6
            width: parent.width - x
            text: noteRow.line
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }

    // Under a rewind's notice: the files it left as they were, and why.
    component LeftRow: Item {
        id: left
        required property int depth
        required property string files
        required property int count
        property bool expanded: false
        readonly property var list: {
            try {
                return JSON.parse(files)
            } catch (e) {
                return []
            }
        }
        width: ListView.view ? ListView.view.width : 0
        height: leftColumn.height + 6
        Column {
            id: leftColumn
            x: left.depth * 18 + 20
            width: parent.width - x
            Item {
                width: parent.width
                height: 28
                Accessible.role: Accessible.Button
                Accessible.name: leftLine.text
                Accessible.onPressAction: left.expanded = !left.expanded
                Row {
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 15
                        height: 15
                        path: left.expanded ? Icons.chevronDown : Icons.chevronRight
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        id: leftLine
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("%n files were left as they are", "", left.count)
                        role: "bodySmall"
                        muted: true
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: left.expanded = !left.expanded
                }
            }
            Repeater {
                model: left.expanded ? left.list : []
                delegate: Row {
                    id: leftFile
                    required property var modelData
                    x: 23
                    height: 22
                    spacing: 10
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(implicitWidth, leftColumn.width - 200)
                        text: leftFile.modelData.path
                        role: "code"
                        size: Theme.typography.bodySmall.size
                        elide: Text.ElideLeft
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: leftFile.modelData.why
                        role: "bodySmall"
                        muted: true
                    }
                }
            }
        }
    }

    // Under an opened step: its whole command, its change, or the last
    // lines of its output.
    component DetailRow: Item {
        id: detail
        required property string key
        required property int depth
        required property string detailKind
        required property string text
        required property string diff
        required property int edit
        required property int edits
        required property bool cut
        // The command as shell, or the output's error, warning and pass
        // lines, as markup the app built from escaped text ("" when plain).
        required property string styled
        width: ListView.view ? ListView.view.width : 0
        height: box.height + 8
        Rectangle {
            id: box
            x: detail.depth * 18 + 20
            y: 2
            width: parent.width - x
            height: inner.height + 16
            radius: Theme.radiusSm
            color: AgentPalette.codeGround
            Column {
                id: inner
                x: 10
                y: 8
                width: parent.width - 20
                spacing: 6
                Item {
                    visible: detail.detailKind === "command" || detail.edits > 1
                    width: parent.width
                    height: visible ? 18 : 0
                    Row {
                        visible: detail.detailKind === "command" && timeline.folder.length > 0
                        spacing: 6
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13
                            height: 13
                            path: Icons.folder
                            color: Theme.surfaceContentVariant
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: timeline.folder
                            role: "caption"
                            muted: true
                        }
                    }
                    Txt {
                        visible: detail.edits > 1
                        text: qsTr("Edit %1 of %2").arg(detail.edit).arg(detail.edits)
                        role: "caption"
                        muted: true
                    }
                    TextAction {
                        id: copy
                        property bool done: false
                        visible: detail.detailKind === "command"
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        text: done ? qsTr("Copied") : qsTr("Copy")
                        iconPath: done ? Icons.check : Icons.copy
                        iconColor: Theme.surfaceContentVariant
                        onClicked: {
                            if (Agents.copyText(detail.key)) {
                                copy.done = true
                                copiedTimer.restart()
                            }
                        }
                        Timer {
                            id: copiedTimer
                            interval: 1500
                            onTriggered: copy.done = false
                        }
                    }
                }
                Txt {
                    visible: detail.detailKind === "output"
                    text: qsTr("Showing the last lines")
                    role: "caption"
                    muted: true
                }
                TextEdit {
                    visible: detail.detailKind !== "diff"
                    width: parent.width
                    text: detail.styled.length > 0 ? detail.styled : detail.text
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.WrapAnywhere
                    // Markup built by the app from escaped text; never the agent's.
                    textFormat: detail.styled.length > 0 ? TextEdit.RichText : TextEdit.PlainText
                    color: Theme.surfaceContent
                    selectionColor: Theme.selection
                    selectedTextColor: Theme.surfaceContent
                    font.family: Theme.fontMono
                    font.features: Theme.monoFeatures
                    font.pixelSize: Theme.typography.code.size
                    font.weight: detail.detailKind === "command" ? 600 : Theme.typography.code.weight
                }
                AgentDiffView {
                    visible: detail.detailKind === "diff"
                    width: parent.width
                    height: visible ? implicitHeight : 0
                    diff: detail.detailKind === "diff" ? detail.diff : ""
                }
            }
        }
    }

    delegate: DelegateChooser {
        role: "view"
        DelegateChoice {
            roleValue: "user"
            delegate: UserRow {}
        }
        DelegateChoice {
            roleValue: "block"
            delegate: AgentBlock {
                agentName: timeline.agentName
                highlightItem: timeline.highlightItem
                onLinkActivated: (key, index) => timeline.linkActivated(key, index)
                onForkRequested: (turn) => timeline.forkFrom(turn)
            }
        }
        DelegateChoice {
            roleValue: "code"
            delegate: AgentCode {}
        }
        DelegateChoice {
            roleValue: "table"
            delegate: AgentTable {
                onLinkActivated: (key, index) => timeline.linkActivated(key, index)
            }
        }
        DelegateChoice {
            roleValue: "tool"
            delegate: AgentToolLine {}
        }
        DelegateChoice {
            roleValue: "detail"
            delegate: DetailRow {}
        }
        DelegateChoice {
            roleValue: "group"
            delegate: AgentGroupLine {}
        }
        DelegateChoice {
            roleValue: "thought"
            delegate: AgentThoughtLine {}
        }
        DelegateChoice {
            roleValue: "fold"
            delegate: AgentFoldLine {}
        }
        DelegateChoice {
            roleValue: "changes"
            delegate: AgentChanges {
                onOpenDiff: (turn, path) => timeline.openChanges(turn, path)
            }
        }
        DelegateChoice {
            roleValue: "changes_note"
            delegate: NoteRow {}
        }
        DelegateChoice {
            roleValue: "left"
            delegate: LeftRow {}
        }
        DelegateChoice {
            roleValue: "mark"
            delegate: AgentMarkLine {
                onWaitingClicked: timeline.waitingClicked()
            }
        }
        DelegateChoice {
            roleValue: "notice"
            delegate: NoticeRow {}
        }
        DelegateChoice {
            roleValue: "plan"
            delegate: AgentPlanCard {
                running: timeline.running
            }
        }
        DelegateChoice {
            roleValue: "plan_foot"
            delegate: AgentPlanCard {
                running: timeline.running
                onImplement: (itemId, mode) => timeline.implementPlan(itemId, mode)
                onImplementMenuRequested: (itemId, x, y) => timeline.implementMenuRequested(itemId, x, y)
                onChangePlan: timeline.changePlan()
            }
        }
        DelegateChoice {
            roleValue: "question"
            delegate: QuestionRow {}
        }
        DelegateChoice {
            roleValue: "live"
            delegate: AgentLiveLine {
                now: timeline.now
                animate: timeline.animate
            }
        }
    }

    // Scrolled up while it works: one click back to the newest.
    Rectangle {
        visible: !timeline.follow && timeline.count > 0 && !timeline.atYEnd
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 12
        width: jumpRow.implicitWidth + 20
        height: 28
        radius: 14
        color: Theme.surfaceContainerHighest
        border.width: 1
        border.color: Theme.outlineVariant
        z: 5
        Accessible.role: Accessible.Button
        Accessible.name: jumpText.text
        Accessible.onPressAction: timeline.jumpToLatest()
        Row {
            id: jumpRow
            anchors.centerIn: parent
            spacing: 6
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: Icons.chevronDown
                color: Theme.surfaceContent
            }
            Txt {
                id: jumpText
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Jump to latest")
                role: "bodySmall"
            }
        }
        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: timeline.jumpToLatest()
        }
    }

    TouchpadGlide { view: timeline }
    // On the view itself: declared inside a ListView it would land in the moving content,
    // stretch to the whole thread's height and lie over the text.
    G7ScrollBar {
        flickable: timeline
        parent: timeline
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.right: parent.right
    }
}
