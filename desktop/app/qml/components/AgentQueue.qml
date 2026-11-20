// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Messages waiting for the turn to end, pinned on top of the composer: up
// to five, one line each, with Send now, edit and remove. After a turn
// that didn't finish they're held: nothing goes until you say.
Rectangle {
    id: queue
    // `composer.queue`: `{ items: [{ id, text, full, from, images }], held }`.
    property var waiting: ({})
    property string agentName
    property bool busy: false
    signal sendNow(string id)
    signal remove(string id)
    signal edit(string id, string text)

    readonly property var items: waiting.items || []
    readonly property bool held: waiting.held === true
    property bool expanded: true
    // The message being edited ("" for none).
    property string editing: ""
    // Tabler's stack-2 (MIT, as Icons.qml credits).
    readonly property string stackIcon: "M12 4l-8 4 8 4 8-4-8-4M4 12l8 4 8-4M4 16l8 4 8-4"

    visible: items.length > 0
    implicitHeight: visible ? column.implicitHeight + 8 : 0
    color: Theme.surfaceContainerLow
    radius: Theme.rowRadius

    Column {
        id: column
        x: 8
        y: 4
        width: parent.width - 16
        Item {
            width: parent.width
            height: 28
            Row {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                    path: queue.expanded ? Icons.chevronDown : Icons.chevronRight
                    color: Theme.surfaceContentVariant
                }
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 15
                    height: 15
                    path: queue.stackIcon
                    color: Theme.surfaceContentVariant
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("%n queued", "", queue.items.length)
                    role: "bodySmall"
                    weight: 600
                }
            }
            Txt {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: queue.held ? qsTr("Waiting: the last turn didn't finish.")
                                 : qsTr("Sent when %1 finishes.").arg(queue.agentName)
                role: "bodySmall"
                color: queue.held ? Theme.tintAmber : Theme.surfaceContentVariant
            }
            MouseArea {
                width: 160
                height: parent.height
                cursorShape: Qt.PointingHandCursor
                onClicked: queue.expanded = !queue.expanded
            }
        }
        Repeater {
            model: queue.expanded ? queue.items : []
            delegate: Item {
                id: line
                required property var modelData
                readonly property bool editing: queue.editing === modelData.id
                width: column.width
                height: 32
                Txt {
                    visible: !line.editing
                    x: 20
                    anchors.right: actions.left
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    text: line.modelData.from.length > 0
                        ? qsTr("%1 · from %2").arg(line.modelData.text).arg(line.modelData.from)
                        : line.modelData.text
                    role: "bodySmall"
                    maximumLineCount: 1
                }
                Rectangle {
                    visible: line.editing
                    x: 16
                    anchors.right: actions.left
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    height: 26
                    radius: Theme.radiusSm
                    color: Theme.raised
                    border.width: 1
                    border.color: Theme.focusRing
                    TextInput {
                        id: editInput
                        x: 8
                        width: parent.width - 16
                        anchors.verticalCenter: parent.verticalCenter
                        clip: true
                        font.family: Theme.fontUi
                        font.pixelSize: Theme.typography.bodySmall.size
                        color: Theme.surfaceContent
                        selectionColor: Theme.selection
                        selectedTextColor: Theme.surfaceContent
                        Keys.onEscapePressed: queue.editing = ""
                        onAccepted: {
                            if (text.trim().length > 0)
                                queue.edit(line.modelData.id, text)
                            queue.editing = ""
                        }
                    }
                }
                Row {
                    id: actions
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4
                    Button {
                        anchors.verticalCenter: parent.verticalCenter
                        variant: "tonal"
                        size: "sm"
                        text: qsTr("Send now")
                        enabled: !queue.busy
                        onClicked: queue.sendNow(line.modelData.id)
                    }
                    IconButton {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 28
                        height: 28
                        iconPath: Icons.edit
                        label: qsTr("Edit")
                        enabled: !queue.busy
                        onClicked: {
                            queue.editing = line.modelData.id
                            editInput.text = line.modelData.full
                            editInput.forceActiveFocus()
                        }
                    }
                    IconButton {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 28
                        height: 28
                        iconPath: Icons.trash
                        label: qsTr("Remove")
                        enabled: !queue.busy
                        onClicked: queue.remove(line.modelData.id)
                    }
                }
            }
        }
    }
}
