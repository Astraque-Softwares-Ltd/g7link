// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A text field on the raised surface: an optional glyph, the input, a muted
// placeholder and an optional keycap. `readOnly` makes it a button that
// looks like a field (the header's search, which opens the palette).
FocusScope {
    id: field
    property alias text: input.text
    property alias input: input
    property string placeholder
    property string iconPath
    property string keys
    property bool readOnly: false
    property int pixelSize: Theme.typography.body.size
    property bool centered: false
    signal accepted
    signal pressed

    implicitWidth: 240
    implicitHeight: 32
    activeFocusOnTab: true

    Accessible.role: readOnly ? Accessible.Button : Accessible.EditableText
    Accessible.name: placeholder
    Accessible.onPressAction: if (field.readOnly) field.pressed()
    Keys.onPressed: (event) => {
        if (field.readOnly && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
            field.pressed()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: Theme.raised
        border.width: input.activeFocus || Theme.focusVisible(field) ? 1 : 0
        border.color: Theme.focusRing
    }
    Icon {
        id: glyph
        visible: field.iconPath.length > 0
        x: 10
        anchors.verticalCenter: parent.verticalCenter
        width: 16; height: 16
        path: field.iconPath
        color: Theme.surfaceContentVariant
    }
    TextInput {
        id: input
        anchors.left: glyph.visible ? glyph.right : parent.left
        anchors.leftMargin: glyph.visible ? 8 : 10
        anchors.right: cap.visible ? cap.left : parent.right
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        focus: !field.readOnly
        readOnly: field.readOnly
        enabled: !field.readOnly
        clip: true
        color: Theme.surfaceContent
        selectionColor: Theme.selection
        selectedTextColor: Theme.surfaceContent
        horizontalAlignment: field.centered ? TextInput.AlignHCenter : TextInput.AlignLeft
        font.family: Theme.fontUi
        font.pixelSize: field.pixelSize
        onAccepted: field.accepted()
        Txt {
            anchors.fill: parent
            visible: input.text.length === 0 && !input.preeditText
            text: field.placeholder
            muted: true
            size: field.pixelSize
            horizontalAlignment: field.centered ? Text.AlignHCenter : Text.AlignLeft
        }
    }
    Keycap {
        id: cap
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        keys: field.keys
    }
    MouseArea {
        anchors.fill: parent
        visible: field.readOnly
        cursorShape: Qt.PointingHandCursor
        onClicked: field.pressed()
    }
}
