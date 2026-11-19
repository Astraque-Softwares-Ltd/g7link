// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// One row of an agent's transcript (AgentItems): what the user wrote, what
// the agent answered, a notice, or a tool step with its command or change and
// its output, folded until asked for.
Item {
    id: entry
    required property int index
    required property string seq
    required property string itemId
    required property string kind
    required property string text
    required property bool truncated
    required property string from
    required property string toolName
    required property string toolTitle
    required property string toolDetail
    required property string toolStatus
    required property string toolOutput
    required property bool streaming
    required property real whenMs

    readonly property bool isEdit: ["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"].indexOf(toolName) >= 0
    readonly property bool isCommand: ["Bash", "shell", "exec", "local_shell"].indexOf(toolName) >= 0
    readonly property var diffLines: isEdit ? toolDetail.split("\n").slice(0, 400) : []
    readonly property int added: diffLines.filter((l) => l.startsWith("+") && !l.startsWith("+++")).length
    readonly property int removed: diffLines.filter((l) => l.startsWith("-") && !l.startsWith("---")).length
    readonly property int outputLines: toolOutput.length > 0 ? toolOutput.split("\n").length : 0
    property bool showDiff: false
    property bool showOutput: false

    width: ListView.view ? ListView.view.width : 0
    height: body.height + 14

    Accessible.role: Accessible.ListItem
    Accessible.name: entry.kind === "tool" ? entry.toolTitle : entry.text

    readonly property string toolIcon: {
        if (isCommand)
            return Icons.terminal
        if (isEdit)
            return Icons.edit
        switch (toolName) {
        case "Read":
        case "NotebookRead": return Icons.text
        case "Grep":
        case "Glob":
        case "LS": return Icons.search
        case "WebFetch":
        case "WebSearch": return Icons.globe
        case "TodoWrite": return Icons.clipboardList
        default: return Icons.bolt
        }
    }

    Column {
        id: body
        x: 0
        y: 7
        width: parent.width
        spacing: 6

        // ---- What the user wrote: on the right, in a bubble ----
        Item {
            visible: entry.kind === "user"
            width: parent.width
            height: visible ? bubble.height + (fromText.visible ? fromText.height + 2 : 0) : 0
            Rectangle {
                id: bubble
                anchors.right: parent.right
                width: Math.min(parent.width * 0.78, userText.implicitWidth + 24)
                height: userText.height + 16
                radius: Theme.radiusMd
                color: Theme.raised
                TextEdit {
                    id: userText
                    x: 12
                    y: 8
                    width: Math.min(entry.width * 0.78 - 24, implicitWidth)
                    text: entry.text
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
            Txt {
                id: fromText
                visible: entry.from.length > 0
                anchors.right: parent.right
                anchors.top: bubble.bottom
                anchors.topMargin: 2
                text: qsTr("From %1").arg(entry.from)
                role: "caption"
                muted: true
            }
        }

        // ---- What the agent answered ----
        Item {
            visible: entry.kind === "assistant"
            width: parent.width
            height: visible ? Math.max(answerText.height, 18) : 0
            Rectangle {
                x: 6
                y: 7
                width: 5
                height: 5
                radius: 2.5
                color: entry.streaming ? Theme.tintBlue : Theme.surfaceContentVariant
            }
            TextEdit {
                id: answerText
                x: 26
                width: parent.width - 26
                text: entry.text
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

        // ---- A notice from G7 (an answer given, a stop, a continue) ----
        Row {
            visible: entry.kind === "notice"
            width: parent.width
            height: visible ? implicitHeight : 0
            spacing: 8
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 16
                height: 16
                path: Icons.info
                color: Theme.surfaceContentVariant
            }
            Txt {
                width: parent.width - 24
                text: entry.text
                role: "bodySmall"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
        }

        // ---- A tool step ----
        Column {
            visible: entry.kind === "tool"
            width: parent.width
            height: visible ? implicitHeight : 0
            spacing: 6

            Item {
                width: parent.width
                height: 22
                Icon {
                    x: 2
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16
                    height: 16
                    path: entry.toolIcon
                    color: entry.toolStatus === "error" ? Theme.error : Theme.surfaceContentVariant
                }
                Txt {
                    anchors.left: parent.left
                    anchors.leftMargin: 26
                    anchors.right: trailing.left
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    text: entry.toolTitle
                    maximumLineCount: 1
                }
                Row {
                    id: trailing
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 6
                    Spinner {
                        visible: entry.toolStatus === "running"
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                    }
                    Txt {
                        visible: entry.isEdit && entry.toolStatus === "ok"
                        anchors.verticalCenter: parent.verticalCenter
                        text: "+" + entry.added
                        role: "bodySmall"
                        color: Theme.tintGreen
                    }
                    Txt {
                        visible: entry.isEdit && entry.toolStatus === "ok"
                        anchors.verticalCenter: parent.verticalCenter
                        text: "−" + entry.removed
                        role: "bodySmall"
                        color: Theme.error
                    }
                    Txt {
                        visible: entry.toolStatus === "error" || entry.toolStatus === "denied"
                        anchors.verticalCenter: parent.verticalCenter
                        text: entry.toolStatus === "error" ? qsTr("Failed") : qsTr("Denied")
                        role: "bodySmall"
                        color: entry.toolStatus === "error" ? Theme.error : Theme.surfaceContentVariant
                    }
                    IconButton {
                        visible: entry.isEdit && entry.toolDetail.length > 0
                        anchors.verticalCenter: parent.verticalCenter
                        width: 24
                        height: 24
                        iconPath: entry.showDiff ? Icons.chevronUp : Icons.chevronDown
                        label: entry.showDiff ? qsTr("Hide the change") : qsTr("Show the change")
                        onClicked: entry.showDiff = !entry.showDiff
                    }
                }
            }

            // The command it runs, in full.
            Rectangle {
                visible: entry.isCommand && entry.toolDetail.length > 0
                x: 26
                width: parent.width - 26
                height: visible ? commandText.height + 16 : 0
                radius: Theme.radiusSm
                color: Theme.surfaceContainerHigh
                Txt {
                    id: commandText
                    x: 10
                    y: 8
                    width: parent.width - 20
                    text: entry.toolDetail
                    role: "code"
                    wrapMode: Text.WrapAnywhere
                    elide: Text.ElideNone
                    maximumLineCount: 12
                }
            }

            // Another tool's input, short.
            Txt {
                visible: !entry.isCommand && !entry.isEdit && entry.toolDetail.length > 0
                x: 26
                width: parent.width - 26
                text: entry.toolDetail
                role: "bodySmall"
                muted: true
                wrapMode: Text.WrapAnywhere
                elide: Text.ElideRight
                maximumLineCount: 2
            }

            // The change, line by line, when asked for.
            Rectangle {
                visible: entry.showDiff && entry.isEdit
                x: 26
                width: parent.width - 26
                height: visible ? diffColumn.height + 12 : 0
                radius: Theme.radiusSm
                color: Theme.surfaceContainerHigh
                clip: true
                Column {
                    id: diffColumn
                    y: 6
                    width: parent.width
                    Repeater {
                        model: entry.showDiff ? entry.diffLines : []
                        delegate: Rectangle {
                            id: diffRow
                            required property string modelData
                            readonly property bool plus: modelData.startsWith("+") && !modelData.startsWith("+++")
                            readonly property bool minus: modelData.startsWith("-") && !modelData.startsWith("---")
                            width: diffColumn.width
                            height: lineText.height
                            color: plus ? Theme.wash(Theme.tintGreen) : (minus ? Theme.wash(Theme.error) : "transparent")
                            Txt {
                                id: lineText
                                x: 10
                                width: parent.width - 20
                                text: diffRow.modelData.length > 0 ? diffRow.modelData : " "
                                role: "code"
                                wrapMode: Text.WrapAnywhere
                                elide: Text.ElideNone
                            }
                        }
                    }
                }
            }

            // Its output, folded.
            Item {
                visible: entry.toolOutput.length > 0
                x: 26
                width: parent.width - 26
                height: visible ? 22 : 0
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Output, %n line(s)", "", entry.outputLines)
                    iconPath: entry.showOutput ? Icons.chevronDown : Icons.chevronRight
                    iconColor: Theme.surfaceContentVariant
                    onClicked: entry.showOutput = !entry.showOutput
                }
            }
            Rectangle {
                visible: entry.showOutput && entry.toolOutput.length > 0
                x: 26
                width: parent.width - 26
                height: visible ? outputText.height + 16 : 0
                radius: Theme.radiusSm
                color: Theme.surfaceContainerHigh
                TextEdit {
                    id: outputText
                    x: 10
                    y: 8
                    width: parent.width - 20
                    text: entry.showOutput ? entry.toolOutput : ""
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.WrapAnywhere
                    textFormat: TextEdit.PlainText
                    color: Theme.surfaceContent
                    selectionColor: Theme.selection
                    selectedTextColor: Theme.surfaceContent
                    font.family: Theme.fontMono
                    font.pixelSize: Theme.typography.code.size
                }
            }
        }

        Txt {
            visible: entry.truncated
            x: entry.kind === "user" ? 0 : 26
            width: parent.width - x
            text: qsTr("Cut short here: it was too long to show in full.")
            role: "caption"
            muted: true
            horizontalAlignment: entry.kind === "user" ? Text.AlignRight : Text.AlignLeft
        }
    }
}
