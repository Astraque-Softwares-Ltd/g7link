// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The commit box (§SC9.1): a message (the first line is the subject; past
// 72 characters a note says so, never a block), "Use the thread's title"
// while it's empty, "Ask <agent> to write it" (a visible message in the
// thread; its answer fills the box unless you typed since), and Commit N
// files, disabled with its reason.
Column {
    id: box
    // Agents.git's commit: { blocked, message: { set, asking, offer }, subjectLong, all }.
    property var commit: ({})
    property string agentName
    property int picked: 0
    // Files in all, when the list was cut at 2,000.
    property int total: 0
    // The thread works or a request runs: asking can't start.
    property bool canAsk: true

    signal edited(string text)
    signal useTitle
    signal ask
    signal useOffer
    signal committed(bool all)

    readonly property var message: commit.message || ({})
    readonly property string blocked: commit.blocked || ""
    readonly property bool hasText: input.text.trim().length > 0
    readonly property bool ready: blocked.length === 0 && hasText
    readonly property string why: {
        switch (blocked) {
        case "nothing": return qsTr("Tick at least one file.")
        case "thread_busy": return message.asking === true ? "" : qsTr("Wait until %1 finishes.").arg(box.agentName)
        case "job": return qsTr("Wait until it finishes.")
        case "git_busy": return qsTr("Git is busy.")
        case "detached": return qsTr("Create a branch first.")
        case "conflicts": return qsTr("Resolve the conflicts first.")
        case "changed": return qsTr("Reload first.")
        case "doing": return qsTr("Finish it in a terminal first.")
        default: return ""
        }
    }

    spacing: 6

    property int seenN: -1
    property bool settingText: false
    readonly property var setText: message.set || ({})
    onSetTextChanged: {
        const n = setText.n ?? 0
        if (n === seenN)
            return
        seenN = n
        settingText = true
        input.text = setText.text || ""
        input.cursorPosition = input.text.length
        settingText = false
    }

    Rectangle {
        width: parent.width
        height: 92
        radius: Theme.radiusSm
        color: Theme.raised
        border.width: 1
        border.color: input.activeFocus ? Theme.focusRing : Theme.outlineVariant
        Flickable {
            id: scroll
            anchors.fill: parent
            anchors.margins: 8
            contentHeight: input.contentHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            TextEdit {
                id: input
                width: scroll.width
                wrapMode: TextEdit.Wrap
                textFormat: TextEdit.PlainText
                font.family: Theme.fontUi
                font.pixelSize: Theme.typography.bodySmall.size
                color: Theme.surfaceContent
                selectionColor: Theme.selection
                selectedTextColor: Theme.surfaceContent
                readOnly: box.message.asking === true
                Accessible.name: qsTr("Commit message")
                onTextChanged: if (!box.settingText) box.edited(text)
                onCursorRectangleChanged: {
                    if (cursorRectangle.y + cursorRectangle.height > scroll.contentY + scroll.height)
                        scroll.contentY = cursorRectangle.y + cursorRectangle.height - scroll.height
                }
                Keys.onPressed: (event) => {
                    if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                        && (event.modifiers & Qt.ControlModifier)) {
                        if (box.ready)
                            box.committed(false)
                        event.accepted = true
                    }
                }
                Txt {
                    visible: input.text.length === 0
                    width: input.width
                    text: box.message.asking === true ? qsTr("Asking %1…").arg(box.agentName) : qsTr("Commit message")
                    role: "bodySmall"
                    muted: true
                }
            }
        }
    }
    Txt {
        visible: box.commit.subjectLong === true
        text: qsTr("Subject is long")
        role: "caption"
        color: Theme.warning
    }
    Flow {
        width: parent.width
        spacing: 2
        TextAction {
            visible: !box.hasText && box.message.asking !== true
            text: qsTr("Use the thread's title")
            onClicked: box.useTitle()
        }
        TextAction {
            visible: box.message.offer === true
            text: qsTr("Use %1's message").arg(box.agentName)
            onClicked: box.useOffer()
        }
        TextAction {
            visible: box.message.asking !== true
            enabled: box.canAsk
            iconPath: AgentGitGlyphs.pencil
            iconColor: Theme.surfaceContentVariant
            text: qsTr("Ask %1 to write it").arg(box.agentName)
            onClicked: box.ask()
        }
    }
    Item {
        width: parent.width
        height: commitButton.height
        Txt {
            anchors.left: parent.left
            anchors.right: commitButton.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            text: box.why
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Button {
            id: commitButton
            anchors.right: parent.right
            size: "sm"
            iconPath: AgentGitGlyphs.commit
            text: box.commit.all === true ? qsTr("Commit all %1 files").arg(box.total) : qsTr("Commit %n files", "", box.picked)
            enabled: box.ready
            onClicked: box.committed(box.commit.all === true)
        }
    }
}
