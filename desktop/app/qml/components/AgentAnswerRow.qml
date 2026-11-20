// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// One answer the agent offers in a question: a dot, or a box when several
// may be picked; its label (the agent's words, escaped, inline code kept)
// and note; the number key that picks it.
Rectangle {
    id: row
    // Markup the app built from the agent's label; or `label`, plain.
    property string styled
    property string label
    property string note
    property bool checked: false
    property bool multi: false
    property int number: 0
    signal picked

    implicitHeight: Math.max(34, column.implicitHeight + 12)
    radius: Theme.radiusSm
    color: row.checked ? Theme.surfaceContainerHigh : (hover.hovered && row.enabled ? Theme.surfaceContainer : "transparent")
    opacity: row.enabled ? 1 : 0.6

    Accessible.role: row.multi ? Accessible.CheckBox : Accessible.RadioButton
    Accessible.name: row.label.length > 0 ? row.label : labelText.text
    Accessible.checked: row.checked
    Accessible.onPressAction: if (row.enabled) row.picked()

    Rectangle {
        x: 10
        y: 10
        width: 16
        height: 16
        radius: row.multi ? 4 : 8
        color: "transparent"
        border.width: 1.5
        border.color: row.checked ? Theme.surfaceContent : Theme.outline
        Rectangle {
            visible: row.checked && !row.multi
            anchors.centerIn: parent
            width: 8
            height: 8
            radius: 4
            color: Theme.surfaceContent
        }
        Icon {
            visible: row.checked && row.multi
            anchors.centerIn: parent
            width: 12
            height: 12
            path: Icons.check
            color: Theme.surfaceContent
        }
    }
    Column {
        id: column
        x: 36
        y: 6
        width: parent.width - x - 40
        spacing: 1
        Text {
            id: labelText
            width: parent.width
            text: row.label.length > 0 ? row.label : row.styled
            // Markup built by the app from escaped text; never the agent's.
            textFormat: row.label.length > 0 ? Text.PlainText : Text.StyledText
            wrapMode: Text.Wrap
            color: Theme.surfaceContent
            linkColor: Theme.accent
            font.family: Theme.fontUi
            font.pixelSize: Theme.typography.body.size
        }
        Txt {
            visible: row.note.length > 0
            width: parent.width
            text: row.note
            role: "caption"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }
    Keycap {
        visible: row.number > 0 && row.number <= 9
        anchors.right: parent.right
        anchors.rightMargin: 10
        y: 9
        keys: String(row.number)
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
