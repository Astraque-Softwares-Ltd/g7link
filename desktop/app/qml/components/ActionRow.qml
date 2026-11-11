// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// One row of the G7 window, 36 px tall: a bare glyph in its tint, a title,
// muted detail after it, and on the right a muted note, a keycap and any
// controls put in `trailing`. Hover and selection are fills only.
FocusScope {
    id: row
    property string iconPath
    property color tint: Theme.surfaceContentVariant
    property string title
    property string detail
    property string meta
    property string keys
    property bool selected: false
    property bool interactive: true
    property bool strong: false
    property color titleColor: Theme.surfaceContent
    property string accessibleName: title
    // Something drawn in place of the glyph (an avatar or an app icon).
    property alias leading: leadingSlot.data
    default property alias trailing: trailingRow.data
    readonly property bool hovered: mouse.containsMouse
    signal clicked
    signal rightClicked(real x, real y)

    implicitHeight: Theme.rowHeight
    implicitWidth: titleText.implicitWidth + rightRow.implicitWidth + 80
    activeFocusOnTab: interactive
    opacity: enabled ? 1 : 0.38

    Accessible.role: interactive ? Accessible.Button : Accessible.StaticText
    Accessible.name: accessibleName
    Accessible.description: detail
    Accessible.onPressAction: if (row.interactive) row.clicked()
    Keys.onPressed: (event) => {
        if (row.interactive && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
            row.clicked()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: row.selected ? Theme.selection : (row.interactive && mouse.containsMouse ? Theme.raised : "transparent")
        border.width: Theme.focusVisible(row) ? 2 : 0
        border.color: Theme.focusRing
    }
    MouseArea {
        id: mouse
        anchors.fill: parent
        enabled: row.interactive && row.enabled
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: row.interactive ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: (m) => {
            if (m.button === Qt.RightButton)
                row.rightClicked(m.x, m.y)
            else
                row.clicked()
        }
    }

    Item {
        id: leadingSlot
        x: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        width: childrenRect.width
        height: childrenRect.height
        visible: children.length > 0
    }
    Icon {
        id: glyph
        visible: row.iconPath.length > 0 && !leadingSlot.visible
        x: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        width: 18; height: 18
        path: row.iconPath
        color: row.tint
    }
    readonly property real textStart: leadingSlot.visible ? leadingSlot.x + leadingSlot.width + 10
        : (glyph.visible ? glyph.x + glyph.width + 10 : Theme.rowPadding)
    readonly property real textRoom: Math.max(0, rightRow.x - 10 - textStart)

    Txt {
        id: titleText
        x: row.textStart
        anchors.verticalCenter: parent.verticalCenter
        width: Math.min(implicitWidth, row.detail.length > 0
            ? Math.max(row.textRoom * 0.5, row.textRoom - detailText.implicitWidth - 10) : row.textRoom)
        text: row.title
        weight: row.strong ? 500 : 400
        color: row.titleColor
    }
    Txt {
        id: detailText
        visible: row.detail.length > 0
        x: titleText.x + titleText.width + (row.title.length > 0 ? 10 : 0)
        anchors.verticalCenter: parent.verticalCenter
        width: Math.max(0, row.textRoom - titleText.width - 10)
        text: row.detail
        muted: true
    }
    Row {
        id: rightRow
        anchors.right: parent.right
        anchors.rightMargin: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8
        Txt {
            visible: row.meta.length > 0
            anchors.verticalCenter: parent.verticalCenter
            text: row.meta
            role: "bodySmall"
            muted: true
        }
        Keycap {
            anchors.verticalCenter: parent.verticalCenter
            keys: row.keys
        }
        Row {
            id: trailingRow
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
        }
    }
}
