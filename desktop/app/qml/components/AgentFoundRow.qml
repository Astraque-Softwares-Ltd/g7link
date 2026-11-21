// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A session found on this PC, in "Bring in earlier sessions": a box to pick
// it, its title, what stands in its way (brought in already, open in a
// terminal now, outside your agent folders, damaged), its agent and date.
// The row itself opens a preview.
FocusScope {
    id: found
    // A row of Agents.import: { id, title, agentName, updatedMs, badge,
    // selectable, selected }.
    property var item: ({})
    property bool selected: false
    property bool busy: false

    signal picked
    signal previewed

    readonly property string badge: item.badge || ""
    readonly property bool selectable: item.selectable === true && !busy
    readonly property string badgeText: {
        switch (badge) {
        case "in": return qsTr("Brought in")
        case "open": return qsTr("Open in a terminal now")
        case "outside": return qsTr("Outside your agent folders")
        case "damaged": return qsTr("Damaged")
        default: return ""
        }
    }
    readonly property string badgeIcon: {
        switch (badge) {
        case "in": return Icons.check
        case "open": return Icons.terminal
        case "outside": return Icons.folder
        case "damaged": return Icons.warning
        default: return ""
        }
    }
    function date(ms) {
        const when = new Date(ms)
        return new Date().toDateString() === when.toDateString()
            ? when.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
            : when.toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }

    implicitHeight: 32
    activeFocusOnTab: true
    Accessible.role: Accessible.ListItem
    Accessible.name: item.title || ""
    Accessible.description: [badgeText, item.agentName].filter((s) => (s || "").length > 0).join(", ")
    Accessible.onPressAction: found.previewed()
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Space && found.selectable) {
            found.picked()
            event.accepted = true
        } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            found.previewed()
            event.accepted = true
        }
    }

    StateFill {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: Theme.raised
        on: !found.selected && !found.item.selected && rowMouse.containsMouse
    }
    StateFill {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: Theme.selection
        on: found.selected || found.item.selected === true
    }
    Rectangle {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: "transparent"
        visible: Theme.focusVisible(found)
        border.width: 2
        border.color: Theme.focusRing
    }
    MouseArea {
        id: rowMouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: found.previewed()
    }

    Rectangle {
        id: box
        x: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        width: 14
        height: 14
        radius: 3
        opacity: found.selectable ? 1 : 0.4
        color: found.item.selected === true ? Theme.accent : "transparent"
        border.width: found.item.selected === true ? 0 : 1.2
        border.color: Theme.outline
        Accessible.role: Accessible.CheckBox
        Accessible.checked: found.item.selected === true
        Accessible.name: qsTr("Select %1").arg(found.item.title || "")
        Icon {
            visible: found.item.selected === true
            anchors.centerIn: parent
            width: 11
            height: 11
            path: Icons.check
            color: Theme.accentContent
        }
        MouseArea {
            anchors.fill: parent
            anchors.margins: -6
            enabled: found.selectable
            cursorShape: Qt.PointingHandCursor
            onClicked: found.picked()
        }
    }
    Txt {
        anchors.left: box.right
        anchors.leftMargin: 10
        anchors.right: badgeRow.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        text: found.item.title || ""
        muted: !found.selectable
        maximumLineCount: 1
    }
    Row {
        id: badgeRow
        anchors.right: agentText.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        width: found.width > 640 ? 190 : (found.badgeText.length > 0 ? 18 : 0)
        spacing: 6
        Icon {
            visible: found.badgeIcon.length > 0
            anchors.verticalCenter: parent.verticalCenter
            width: 13
            height: 13
            path: found.badgeIcon
            color: found.badge === "damaged" ? AgentPalette.failed
                : found.badge === "open" ? AgentPalette.needsYou : Theme.surfaceContentVariant
        }
        Txt {
            visible: found.width > 640
            width: parent.width - 19
            anchors.verticalCenter: parent.verticalCenter
            text: found.badgeText
            role: "bodySmall"
            color: found.badge === "damaged" ? AgentPalette.failedText : Theme.surfaceContent
            muted: found.badge !== "damaged" && found.badge !== "open"
            maximumLineCount: 1
        }
    }
    Txt {
        id: agentText
        anchors.right: dateText.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        width: 48
        text: found.item.agentName || ""
        role: "bodySmall"
        muted: true
    }
    Txt {
        id: dateText
        anchors.right: parent.right
        anchors.rightMargin: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        width: 86
        horizontalAlignment: Text.AlignRight
        text: found.date(found.item.updatedMs || 0)
        role: "code"
        size: Theme.typography.caption.size
        muted: true
    }
}
