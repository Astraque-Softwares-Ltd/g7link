// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The files of a diff: each one's status letter, path (cut at the start so
// the name stays) and lines added and removed; the one shown is selected.
// Past 500 files the rest is counted. Ctrl+J and Ctrl+K step through them.
Item {
    id: list
    // Agents.diff's files: [{ path, from, status, adds, dels, binary }].
    property var files: []
    property int selected: 0
    // Files changed but not listed.
    property int more: 0

    signal picked(int index)

    // "M", "A", "D", "R", "T" and "?" for one that wasn't recorded.
    function letter(status) {
        switch (status) {
        case "added": return "A"
        case "deleted": return "D"
        case "renamed": return "R"
        case "type": return "T"
        case "unrecorded": return "?"
        default: return "M"
        }
    }
    function tint(status) {
        switch (status) {
        case "added": return AgentPalette.inserted
        case "deleted": return AgentPalette.deleted
        case "renamed": return AgentPalette.working
        case "unrecorded": return Theme.surfaceContentVariant
        default: return Theme.surfaceContent
        }
    }

    ListView {
        id: view
        anchors.fill: parent
        anchors.margins: 8
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: list.files
        currentIndex: list.selected
        header: Item {
            width: view.width
            height: 26
            Txt {
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("%n files changed", "", list.files.length + list.more)
                role: "label"
                muted: true
            }
        }
        footer: Txt {
            visible: list.more > 0
            x: 8
            height: visible ? 26 : 0
            verticalAlignment: Text.AlignVCenter
            text: qsTr("and %1 more").arg(list.more)
            role: "bodySmall"
            muted: true
        }
        delegate: Item {
            id: file
            required property var modelData
            required property int index
            readonly property bool current: index === list.selected
            width: view.width
            height: 28
            activeFocusOnTab: true
            Accessible.role: Accessible.ListItem
            Accessible.name: modelData.path
            Accessible.selected: current
            Accessible.onPressAction: list.picked(file.index)
            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                    list.picked(file.index)
                    event.accepted = true
                }
            }
            StateFill {
                anchors.fill: parent
                radius: Theme.radiusSm
                color: Theme.raised
                on: !file.current && fileMouse.containsMouse
            }
            StateFill {
                anchors.fill: parent
                radius: Theme.radiusSm
                color: Theme.selection
                on: file.current
            }
            Rectangle {
                anchors.fill: parent
                radius: Theme.radiusSm
                color: "transparent"
                visible: Theme.focusVisible(file)
                border.width: 2
                border.color: Theme.focusRing
            }
            Txt {
                id: statusLetter
                x: 8
                width: 12
                anchors.verticalCenter: parent.verticalCenter
                text: list.letter(file.modelData.status)
                role: "code"
                size: Theme.typography.bodySmall.size
                color: list.tint(file.modelData.status)
            }
            Txt {
                anchors.left: statusLetter.right
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
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                visible: !file.modelData.binary && file.modelData.status !== "unrecorded"
                Txt {
                    text: "+" + file.modelData.adds
                    role: "caption"
                    color: AgentPalette.inserted
                }
                Txt {
                    text: "−" + file.modelData.dels
                    role: "caption"
                    color: AgentPalette.deleted
                }
            }
            MouseArea {
                id: fileMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: list.picked(file.index)
            }
        }
        TouchpadGlide { view: view }
    }
    G7ScrollBar {
        flickable: view
        anchors.top: view.top
        anchors.bottom: view.bottom
        anchors.right: parent.right
    }
}
