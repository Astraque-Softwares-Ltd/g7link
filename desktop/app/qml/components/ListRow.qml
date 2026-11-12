// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A settings-style row: optional icon, title and description, and a
// trailing control placed in `trailing`.
Item {
    id: row
    property string iconPath
    property string title
    property string description
    property bool stackBelow: false
    // The icon's tint (G7 theme: each setting's glyph in its own color).
    property color iconColor: Theme.surfaceContentVariant
    default property alias trailing: trailingSlot.data

    readonly property bool stackTrailing: stackBelow || (trailingSlot.childrenRect.width > row.width * 0.52 && row.width > 0)

    implicitHeight: stackTrailing
        ? texts.implicitHeight + trailingSlot.childrenRect.height + 34
        : Math.max(Theme.rowHeight, texts.implicitHeight + 14, trailingSlot.childrenRect.height + 10)

    Icon {
        id: icon
        visible: row.iconPath.length > 0
        anchors.left: parent.left
        // On the title's first line, however many lines the text wraps to.
        y: texts.y + Math.round((titleText.lineHeight - height) / 2)
        width: 18
        height: width
        path: row.iconPath
        color: row.iconColor
    }
    Column {
        id: texts
        anchors.left: icon.visible ? icon.right : parent.left
        anchors.leftMargin: icon.visible ? 12 : 0
        anchors.right: row.stackTrailing ? parent.right : trailingSlot.left
        anchors.rightMargin: row.stackTrailing ? 0 : 16
        y: row.stackTrailing ? 12 : Math.round((row.height - height) / 2)
        spacing: 2
        Txt {
            id: titleText
            width: parent.width
            text: row.title
            role: "body"
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Txt {
            visible: row.description.length > 0
            width: parent.width
            text: row.description
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }
    Item {
        id: trailingSlot
        anchors.left: row.stackTrailing ? texts.left : undefined
        anchors.right: row.stackTrailing ? undefined : parent.right
        y: row.stackTrailing ? (texts.y + texts.implicitHeight + 10) : Math.round((row.height - height) / 2)
        width: childrenRect.width
        height: childrenRect.height
    }
}
