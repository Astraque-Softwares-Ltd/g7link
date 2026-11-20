// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// One choice in the model control: a check when it's the one, its name, a
// muted line under it, and a tag on the right ("Default", a count).
Rectangle {
    id: row
    property string label
    property string note
    property string tag
    property bool checked: false
    // A row that opens more rows ("More models").
    property bool opens: false
    // Drawn as code (a model's id typed by hand).
    property bool mono: false
    signal picked

    implicitHeight: Math.max(34, column.implicitHeight + 12)
    radius: Theme.radiusSm
    color: row.checked ? Theme.surfaceContainerHigh : (hover.hovered && row.enabled ? Theme.surfaceContainer : "transparent")
    opacity: row.enabled ? 1 : 0.55

    Accessible.role: Accessible.RadioButton
    Accessible.name: row.label
    Accessible.checked: row.checked
    Accessible.onPressAction: if (row.enabled) row.picked()

    Icon {
        visible: row.checked
        x: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 14
        height: 14
        path: Icons.check
        color: Theme.surfaceContent
    }
    Column {
        id: column
        x: 30
        anchors.verticalCenter: parent.verticalCenter
        width: parent.width - x - tagText.width - (row.opens ? 26 : 12)
        spacing: 1
        Txt {
            width: parent.width
            text: row.label
            role: row.mono ? "code" : "body"
            maximumLineCount: 1
        }
        Txt {
            visible: row.note.length > 0
            width: parent.width
            text: row.note
            role: "caption"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
            maximumLineCount: 2
        }
    }
    Txt {
        id: tagText
        anchors.right: row.opens ? chevron.left : parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        text: row.tag
        role: "caption"
        muted: true
    }
    Icon {
        id: chevron
        visible: row.opens
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 14
        height: 14
        path: Icons.chevronRight
        color: Theme.surfaceContentVariant
    }
    HoverHandler {
        id: hover
    }
    MouseArea {
        anchors.fill: parent
        enabled: row.enabled
        cursorShape: Qt.PointingHandCursor
        onClicked: row.picked()
    }
}
