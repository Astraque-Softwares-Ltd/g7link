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

    function jumpToLatest() {
        follow = true
        positionViewAtEnd()
    }

    model: AgentTimeline
    clip: true
    spacing: 0
    boundsBehavior: Flickable.StopAtBounds
    // Rows past the view are laid out ahead, so heights settle before they show.
    cacheBuffer: 1200
    onMovementEnded: follow = atYEnd
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
        width: ListView.view ? ListView.view.width : 0
        height: column.height + 14
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
                color: Theme.raised
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
                color: asked.status === "asked" ? Theme.tintAmber : Theme.surfaceContentVariant
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
        id: notice
        required property string text
        required property int depth
        width: ListView.view ? ListView.view.width : 0
        height: noticeText.height + 12
        Icon {
            x: notice.depth * 18 + 20
            y: 7
            width: 16
            height: 16
            path: Icons.info
            color: Theme.surfaceContentVariant
        }
        Rectangle {
            visible: notice.depth > 0
            x: (notice.depth - 1) * 18 + 8
            width: 1
            height: parent.height
            color: Theme.line
        }
        Txt {
            id: noticeText
            x: notice.depth * 18 + 44
            y: 6
            width: parent.width - x
            text: notice.text
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
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
        width: ListView.view ? ListView.view.width : 0
        height: box.height + 8
        Rectangle {
            id: box
            x: detail.depth * 18 + 20
            y: 2
            width: parent.width - x
            height: inner.height + 16
            radius: Theme.radiusSm
            color: Theme.surfaceContainerHigh
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
                    text: detail.text
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.WrapAnywhere
                    textFormat: TextEdit.PlainText
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
                onLinkActivated: (key, index) => timeline.linkActivated(key, index)
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
            delegate: AgentChanges {}
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
    G7ScrollBar {
        flickable: timeline
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.right: parent.right
    }
}
