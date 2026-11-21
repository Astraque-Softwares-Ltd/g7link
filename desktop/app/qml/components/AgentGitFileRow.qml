// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// One changed file in the source-control panel: its tick, the status letter
// the diff view uses (with its word as the accessible label), the path cut
// at the start so the name stays, and lines added and removed. The path
// opens what isn't committed at this file.
Item {
    id: row
    // { path, from, status, folder, adds, dels, binary, ticked, conflict }
    property var file: ({})
    // Ticks can change (no job runs).
    property bool editable: true
    signal ticked(bool on)
    signal opened

    readonly property string letter: {
        switch (file.status) {
        case "added":
        case "untracked": return "A"
        case "deleted": return "D"
        case "renamed": return "R"
        case "conflict": return "C"
        case "type": return "T"
        default: return "M"
        }
    }
    readonly property string word: {
        switch (file.status) {
        case "added": return qsTr("Added")
        case "untracked": return file.folder === true ? qsTr("New folder") : qsTr("New")
        case "deleted": return qsTr("Deleted")
        case "renamed": return qsTr("Renamed")
        case "conflict": return qsTr("Conflict")
        case "type": return qsTr("Type changed")
        default: return qsTr("Changed")
        }
    }
    readonly property color tint: {
        switch (file.status) {
        case "added":
        case "untracked": return AgentPalette.inserted
        case "deleted":
        case "conflict": return AgentPalette.deleted
        case "renamed": return AgentPalette.working
        default: return AgentPalette.needsYou
        }
    }

    height: 28

    Rectangle {
        anchors.fill: parent
        radius: Theme.radiusSm
        color: rowHover.hovered ? Theme.selection : "transparent"
    }
    Rectangle {
        id: box
        x: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 14
        height: 14
        radius: 3
        color: row.file.ticked === true ? Theme.accent : "transparent"
        border.width: row.file.ticked === true ? 0 : 1
        border.color: box.activeFocus ? Theme.focusRing : Theme.outline
        opacity: row.editable && row.file.conflict !== true ? 1 : 0.38
        activeFocusOnTab: row.editable && row.file.conflict !== true
        Accessible.role: Accessible.CheckBox
        Accessible.name: qsTr("Include %1").arg(row.file.path || "")
        Accessible.checkable: true
        Accessible.checked: row.file.ticked === true
        Accessible.onToggleAction: row.ticked(row.file.ticked !== true)
        Accessible.onPressAction: row.ticked(row.file.ticked !== true)
        Keys.onSpacePressed: row.ticked(row.file.ticked !== true)
        Icon {
            visible: row.file.ticked === true
            anchors.centerIn: parent
            width: 11
            height: 11
            path: Icons.check
            color: Theme.accentContent
            stroke: 2.4
        }
        TapHandler {
            enabled: row.editable && row.file.conflict !== true
            onTapped: row.ticked(row.file.ticked !== true)
        }
    }
    Txt {
        id: letterText
        anchors.left: box.right
        anchors.leftMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 10
        text: row.letter
        role: "code"
        size: Theme.typography.bodySmall.size
        color: row.tint
        Accessible.role: Accessible.StaticText
        Accessible.name: row.word
    }
    Txt {
        id: pathText
        anchors.left: letterText.right
        anchors.leftMargin: 8
        anchors.right: counts.left
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        text: (row.file.from || "").length > 0 ? row.file.from + " → " + row.file.path : (row.file.path || "")
        role: "code"
        size: Theme.typography.bodySmall.size
        elide: Text.ElideLeft
        maximumLineCount: 1
        Accessible.role: Accessible.Link
        Accessible.name: text
        Accessible.onPressAction: row.opened()
        TapHandler {
            onTapped: row.opened()
        }
        HoverHandler {
            cursorShape: Qt.PointingHandCursor
        }
    }
    Row {
        id: counts
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 6
        visible: (row.file.adds ?? -1) >= 0 && row.file.binary !== true
        Txt {
            text: "+" + (row.file.adds || 0)
            role: "code"
            size: Theme.typography.caption.size
            color: AgentPalette.inserted
        }
        Txt {
            text: "−" + (row.file.dels || 0)
            role: "code"
            size: Theme.typography.caption.size
            color: AgentPalette.deleted
        }
    }
    HoverHandler {
        id: rowHover
    }
}
