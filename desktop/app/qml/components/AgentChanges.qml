// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// "3 files changed +26 −7" under a finished turn's answer: each file opens
// to that turn's edits of it.
Item {
    id: changes
    required property int depth
    required property int turn
    required property string files
    required property int adds
    required property int dels
    required property int more

    readonly property var list: {
        try {
            return JSON.parse(files)
        } catch (e) {
            return []
        }
    }

    width: ListView.view ? ListView.view.width : 0
    height: card.height + 8

    Rectangle {
        id: card
        x: changes.depth * 18
        y: 4
        width: parent.width - x
        height: column.height + 12
        radius: Theme.radiusMd
        color: Theme.surfaceContainerLow
        border.width: 1
        border.color: Theme.outlineVariant
        Column {
            id: column
            x: 6
            y: 6
            width: parent.width - 12
            Item {
                width: parent.width
                height: 30
                Row {
                    x: 6
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16
                        height: 16
                        path: Icons.edit
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("%n file(s) changed", "", changes.list.length + changes.more)
                        role: "bodySmall"
                        weight: 600
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "+" + changes.adds
                        role: "bodySmall"
                        color: Theme.tintGreen
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "−" + changes.dels
                        role: "bodySmall"
                        color: Theme.error
                    }
                }
            }
            Repeater {
                model: changes.list
                delegate: Item {
                    id: file
                    required property var modelData
                    required property int index
                    readonly property string fileKey: "c:" + changes.turn + ":" + index
                    width: column.width
                    height: 26
                    Accessible.role: Accessible.Button
                    Accessible.name: modelData.path
                    Accessible.onPressAction: Agents.toggle(file.fileKey)
                    Rectangle {
                        anchors.fill: parent
                        radius: Theme.radiusSm
                        color: file.modelData.open ? Theme.selection : Theme.surfaceContentVariant
                        opacity: file.modelData.open ? 1 : (fileHover.hovered ? 0.06 : 0)
                    }
                    Icon {
                        id: chevron
                        x: 6
                        anchors.verticalCenter: parent.verticalCenter
                        width: 13
                        height: 13
                        path: file.modelData.open ? Icons.chevronDown : Icons.chevronRight
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        anchors.left: chevron.right
                        anchors.leftMargin: 8
                        anchors.right: counts.left
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        text: file.modelData.path
                        role: "code"
                        size: Theme.typography.bodySmall.size
                        elide: Text.ElideLeft
                    }
                    Row {
                        id: counts
                        anchors.right: parent.right
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Txt {
                            text: "+" + file.modelData.adds
                            role: "caption"
                            color: Theme.tintGreen
                        }
                        Txt {
                            text: "−" + file.modelData.dels
                            role: "caption"
                            color: Theme.error
                        }
                    }
                    HoverHandler {
                        id: fileHover
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: Agents.toggle(file.fileKey)
                    }
                }
            }
            Txt {
                visible: changes.more > 0
                x: 6
                height: visible ? 24 : 0
                text: qsTr("%n more file(s)", "", changes.more)
                role: "caption"
                muted: true
            }
        }
    }
}
