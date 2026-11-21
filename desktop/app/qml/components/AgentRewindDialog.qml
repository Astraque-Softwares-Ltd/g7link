// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Rewind to before a message: the one place files are changed. It says
// exactly what goes back: the later messages that leave the thread, the
// files changed back, deleted (created after that point) and brought back;
// changes made after the agent finished are listed apart and left alone
// unless ticked. Cancel has the keyboard; Rewind takes a click, or Tab then
// Enter, never a bare Enter. Undo puts everything back until the next
// message is sent.
Sheet {
    id: dialog
    // Agents.rewind, parsed ({} while it's closed).
    property var rewind: ({})

    readonly property bool shown: rewind.turn !== undefined
    readonly property var can: rewind.can || ({})
    readonly property string choice: rewind.choice || "conversation"
    readonly property bool withFiles: choice !== "conversation" && rewind.files === true
    readonly property bool withConversation: choice !== "files"

    // Why the files, or the conversation, can't go back.
    function reason(word) {
        switch (word) {
        case "shared": return qsTr("Another thread worked in this folder since then.")
        case "busy": return qsTr("Git is in the middle of something in this folder. Finish it, then try again.")
        case "no_checkpoint":
        case "no_restore_point": return qsTr("No restore point for this turn.")
        case "not_git": return qsTr("This isn't a git repository, so G7 can't undo file changes here.")
        case "expired": return qsTr("This restore point is no longer kept.")
        case "off": return qsTr("Restore points are off for this folder.")
        case "agent": return qsTr("%1 can only be forked from its latest message.").arg(rewind.agentName || "")
        default: return ""
        }
    }
    readonly property string blockedLine: {
        if (can.both !== false || !shown)
            return ""
        return reason((rewind.blocked || "").length > 0 ? rewind.blocked : (rewind.filesWhy || ""))
    }

    cardWidth: 560
    dismissable: rewind.applying !== true
    onShownChanged: {
        if (shown) {
            open()
            Qt.callLater(() => cancelButton.forceActiveFocus(Qt.OtherFocusReason))
        } else {
            close()
        }
    }
    onDismissed: Agents.rewindClose()

    // A list of the plan: its title, then the paths, collapsed past 8.
    component PathList: Column {
        id: paths
        property string title
        property string note
        property var list: []
        property bool expanded: false
        visible: list.length > 0
        width: parent ? parent.width : 0
        Item {
            width: paths.width
            height: 28
            Row {
                x: 10
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: paths.title
                    role: "bodySmall"
                    weight: 600
                }
                Txt {
                    visible: paths.note.length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    text: paths.note
                    role: "bodySmall"
                    muted: true
                }
            }
            Txt {
                anchors.right: parent.right
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                text: String(paths.list.length)
                role: "caption"
                muted: true
            }
        }
        Repeater {
            model: paths.expanded ? paths.list : paths.list.slice(0, 8)
            delegate: Txt {
                required property var modelData
                x: 26
                width: paths.width - 36
                height: 22
                verticalAlignment: Text.AlignVCenter
                text: modelData
                role: "code"
                size: Theme.typography.bodySmall.size
                elide: Text.ElideLeft
            }
        }
        TextAction {
            visible: !paths.expanded && paths.list.length > 8
            x: 22
            text: qsTr("and %1 more").arg(paths.list.length - 8)
            iconPath: Icons.chevronDown
            onClicked: paths.expanded = true
        }
    }

    Column {
        width: parent.width
        spacing: 12

        Txt {
            width: parent.width
            text: qsTr("Rewind to before this message?")
            role: "headline"
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Rectangle {
            visible: (dialog.rewind.quote || "").length > 0
            width: parent.width
            height: visible ? quoteText.height + 16 : 0
            radius: Theme.radiusMd
            color: Theme.surfaceContainerLow
            Txt {
                id: quoteText
                x: 12
                y: 8
                width: parent.width - 24
                text: dialog.rewind.quote || ""
                role: "bodySmall"
                muted: true
                maximumLineCount: 1
            }
        }

        // What goes back: a choice, each one disabled when it can't be done.
        Row {
            id: choices
            width: parent.width
            spacing: 4
            Accessible.role: Accessible.PageTabList
            Accessible.name: qsTr("What to rewind")
            Repeater {
                model: [
                    { value: "both", label: qsTr("Conversation and files") },
                    { value: "conversation", label: qsTr("Conversation only") },
                    { value: "files", label: qsTr("Files only") }
                ]
                delegate: Item {
                    id: option
                    required property var modelData
                    readonly property bool on: dialog.choice === modelData.value
                    readonly property bool allowed: dialog.can[modelData.value] === true
                    width: (choices.width - 8) / 3
                    height: 32
                    activeFocusOnTab: allowed
                    Accessible.role: Accessible.PageTab
                    Accessible.name: modelData.label
                    Accessible.checked: on
                    Accessible.onPressAction: if (option.allowed) Agents.rewindChoose(option.modelData.value)
                    Keys.onPressed: (event) => {
                        if (option.allowed && (event.key === Qt.Key_Space || event.key === Qt.Key_Return
                                               || event.key === Qt.Key_Enter)) {
                            Agents.rewindChoose(option.modelData.value)
                            event.accepted = true
                        }
                    }
                    Rectangle {
                        anchors.fill: parent
                        radius: Theme.radiusSm
                        color: option.on ? Theme.selection : Theme.surfaceContainerLow
                        border.width: Theme.focusVisible(option) ? 2 : 0
                        border.color: Theme.focusRing
                    }
                    Txt {
                        anchors.centerIn: parent
                        width: parent.width - 12
                        horizontalAlignment: Text.AlignHCenter
                        text: option.modelData.label
                        role: "bodySmall"
                        weight: option.on ? 600 : 500
                        muted: !option.allowed
                        maximumLineCount: 1
                    }
                    MouseArea {
                        anchors.fill: parent
                        enabled: option.allowed && dialog.rewind.applying !== true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: Agents.rewindChoose(option.modelData.value)
                    }
                }
            }
        }
        Row {
            visible: dialog.blockedLine.length > 0
            width: parent.width
            spacing: 8
            Icon {
                width: 14
                height: 14
                path: Icons.info
                color: Theme.surfaceContentVariant
            }
            Txt {
                width: parent.width - 22
                text: dialog.blockedLine
                role: "bodySmall"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
        }
        Rectangle {
            visible: dialog.rewind.conflict === true
            width: parent.width
            height: visible ? conflictRow.height + 16 : 0
            radius: Theme.radiusMd
            color: "transparent"
            border.width: 1
            border.color: AgentPalette.needsYou
            Row {
                id: conflictRow
                x: 10
                y: 8
                spacing: 8
                Icon {
                    width: 15
                    height: 15
                    path: Icons.warning
                    color: AgentPalette.needsYou
                }
                Txt {
                    text: qsTr("The files changed while you were looking.")
                    role: "bodySmall"
                    weight: 500
                }
            }
        }
        Txt {
            visible: (dialog.rewind.error || "").length > 0
            width: parent.width
            text: dialog.rewind.error || ""
            role: "bodySmall"
            color: AgentPalette.failedText
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Row {
            visible: dialog.rewind.ready !== true && (dialog.rewind.error || "").length === 0
            spacing: 8
            Spinner {
                width: 16
                height: 16
            }
            Txt {
                text: qsTr("Loading…")
                role: "bodySmall"
                muted: true
            }
        }
        Txt {
            visible: dialog.rewind.ready === true && dialog.withConversation && (dialog.rewind.messages || 0) > 0
            width: parent.width
            text: qsTr("%n later messages leave the thread.", "", dialog.rewind.messages || 0)
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }

        // The files, exactly as they will change.
        Rectangle {
            visible: dialog.rewind.ready === true && dialog.withFiles
                     && (dialog.rewind.restore || []).length + (dialog.rewind.delete || []).length
                        + (dialog.rewind.bringBack || []).length > 0
            width: parent.width
            height: visible ? Math.min(lists.height + 10, 300) : 0
            radius: Theme.radiusMd
            color: Theme.surfaceContainerLow
            border.width: 1
            border.color: Theme.outlineVariant
            clip: true
            Flickable {
                id: listsFlick
                anchors.fill: parent
                anchors.topMargin: 4
                anchors.bottomMargin: 6
                contentHeight: lists.height
                boundsBehavior: Flickable.StopAtBounds
                Column {
                    id: lists
                    width: listsFlick.width
                    PathList {
                        title: qsTr("Changed back")
                        list: dialog.rewind.restore || []
                    }
                    PathList {
                        title: qsTr("Deleted")
                        note: qsTr("created after this point")
                        list: dialog.rewind.delete || []
                    }
                    PathList {
                        title: qsTr("Brought back")
                        list: dialog.rewind.bringBack || []
                    }
                    Txt {
                        visible: (dialog.rewind.more || 0) > 0
                        x: 26
                        height: visible ? 22 : 0
                        text: qsTr("and %1 more").arg(dialog.rewind.more || 0)
                        role: "bodySmall"
                        muted: true
                    }
                }
                TouchpadGlide { view: listsFlick }
            }
            G7ScrollBar {
                flickable: listsFlick
                anchors.top: listsFlick.top
                anchors.bottom: listsFlick.bottom
                anchors.right: parent.right
            }
        }

        // Changes made after the agent finished: apart, and left alone unless ticked.
        Column {
            visible: dialog.rewind.ready === true && dialog.withFiles && (dialog.rewind.laterCount || 0) > 0
            width: parent.width
            spacing: 2
            Item {
                width: parent.width
                height: 28
                activeFocusOnTab: true
                Accessible.role: Accessible.CheckBox
                Accessible.checked: dialog.rewind.includeLater === true
                Accessible.name: laterText.text
                Accessible.onPressAction: Agents.rewindLater(dialog.rewind.includeLater !== true)
                Keys.onSpacePressed: Agents.rewindLater(dialog.rewind.includeLater !== true)
                Rectangle {
                    id: laterBox
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16
                    height: 16
                    radius: 3
                    color: dialog.rewind.includeLater === true ? Theme.accent : "transparent"
                    border.width: dialog.rewind.includeLater === true ? 0 : 1.2
                    border.color: Theme.outline
                    Icon {
                        visible: dialog.rewind.includeLater === true
                        anchors.centerIn: parent
                        width: 12
                        height: 12
                        path: Icons.check
                        color: Theme.accentContent
                    }
                }
                Txt {
                    id: laterText
                    anchors.left: laterBox.right
                    anchors.leftMargin: 10
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Also undo %n changes made after %1 finished", "", dialog.rewind.laterCount || 0)
                        .arg(dialog.rewind.agentName || "")
                    maximumLineCount: 1
                }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: Agents.rewindLater(dialog.rewind.includeLater !== true)
                }
            }
            Repeater {
                model: dialog.rewind.later || []
                delegate: Txt {
                    required property var modelData
                    x: 26
                    height: 22
                    verticalAlignment: Text.AlignVCenter
                    text: modelData
                    role: "code"
                    size: Theme.typography.bodySmall.size
                    muted: dialog.rewind.includeLater !== true
                }
            }
        }

        Column {
            visible: dialog.rewind.ready === true && dialog.withFiles
            width: parent.width
            spacing: 4
            Txt {
                width: parent.width
                text: qsTr("Ignored files and files outside %1 aren't touched.").arg(dialog.rewind.folder || "")
                role: "bodySmall"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Txt {
                visible: (dialog.rewind.skipped || 0) > 0
                width: parent.width
                text: qsTr("%n large files weren't recorded and aren't touched.", "", dialog.rewind.skipped || 0)
                role: "bodySmall"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Txt {
                visible: dialog.rewind.headMoved === true
                width: parent.width
                text: qsTr("The branch moved since then and keeps its commits. G7 restores files only, so they will show as changes.")
                role: "bodySmall"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
        }

        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                id: cancelButton
                variant: "text"
                text: qsTr("Cancel")
                enabled: dialog.rewind.applying !== true
                onClicked: Agents.rewindClose()
            }
            // Secondary, on the right: a click, or Tab then Enter.
            Button {
                id: rewindButton
                variant: "tonal"
                text: qsTr("Rewind")
                busy: dialog.rewind.applying === true
                enabled: dialog.rewind.ready === true
                onActiveFocusChanged: Agents.rewindFocus(activeFocus)
                onClicked: Agents.rewindApply(rewindHover.hovered ? "click" : "key")
                HoverHandler {
                    id: rewindHover
                }
            }
        }
    }
}
