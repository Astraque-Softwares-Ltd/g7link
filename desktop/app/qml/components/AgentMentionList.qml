// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// `@` in the composer: files of the thread's folder, as the agent host
// ranks them, up to 8. Enter or Tab inserts one; a folder inserts its path
// and lists what's in it. Nothing is read or attached: the agent gets the path.
Rectangle {
    id: list
    // `Agents.popup` (kind "files"), parsed.
    property var popup: ({})
    signal chosen(int index)

    readonly property var rows: popup.rows || []
    readonly property int selected: popup.selected || 0

    implicitHeight: column.implicitHeight + 12
    radius: Theme.radiusLg
    color: Theme.surface
    border.width: 1
    border.color: Theme.line

    Accessible.role: Accessible.List
    Accessible.name: header.text

    Column {
        id: column
        x: 6
        y: 6
        width: parent.width - 12
        spacing: 2
        Txt {
            id: header
            x: 8
            height: 24
            verticalAlignment: Text.AlignVCenter
            text: qsTr("Files in %1").arg(list.popup.folder || "")
            role: "label"
            muted: true
        }
        Repeater {
            model: list.rows
            delegate: Rectangle {
                id: fileRow
                required property var modelData
                required property int index
                width: column.width
                height: 28
                radius: Theme.radiusSm
                color: fileRow.index === list.selected ? Theme.surfaceContainerHigh : "transparent"
                Accessible.role: Accessible.ListItem
                Accessible.name: fileRow.modelData.path
                Accessible.selected: fileRow.index === list.selected
                Row {
                    x: 8
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 16
                    spacing: 0
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        path: fileRow.modelData.dir ? Icons.folder : Icons.text
                        color: Theme.surfaceContentVariant
                    }
                    Item {
                        width: 8
                        height: 1
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(implicitWidth, parent.width - 22 - nameText.implicitWidth)
                        text: fileRow.modelData.parent
                        role: "code"
                        muted: true
                        elide: Text.ElideLeft
                    }
                    Txt {
                        id: nameText
                        anchors.verticalCenter: parent.verticalCenter
                        text: fileRow.modelData.name
                        role: "code"
                        weight: 600
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: list.chosen(fileRow.index)
                }
            }
        }
        Row {
            visible: list.rows.length === 0
            x: 8
            height: 28
            spacing: 8
            Spinner {
                visible: list.popup.loading === true
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
            }
            Txt {
                visible: list.popup.loading !== true
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("No files match")
                role: "bodySmall"
                muted: true
            }
        }
        Rectangle {
            width: parent.width
            height: 1
            color: Theme.line
        }
        AgentPopupLegend {
            x: 8
            height: 26
        }
    }
}
