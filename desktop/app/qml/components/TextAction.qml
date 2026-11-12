// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A small, quiet text button: "Clear all", "Mark as read", "New". Muted
// until hovered; `selected` gives it the selection fill (filter tabs). An
// `imageSource` (an app's icon) shows in place of the `iconPath` glyph once
// it has loaded; a `badge` (a count) follows the text, muted.
FocusScope {
    id: action
    property string text
    property string iconPath
    property url imageSource
    property string badge
    property color iconColor: Theme.accent
    property bool selected: false
    property bool muted: !selected
    signal clicked

    implicitWidth: content.implicitWidth + 16
    implicitHeight: 24
    activeFocusOnTab: true
    opacity: enabled ? 1 : 0.38

    Accessible.role: Accessible.Button
    Accessible.name: badge.length > 0 ? text + " " + badge : text
    Accessible.onPressAction: if (enabled) action.clicked()
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
            action.clicked()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: Theme.radiusSm
        color: action.selected ? Theme.activeFill : (mouse.containsMouse ? Theme.raised : "transparent")
        border.width: Theme.focusVisible(action) ? 2 : (action.selected ? 1 : 0)
        border.color: Theme.focusVisible(action) ? Theme.focusRing : Theme.accent
    }
    Row {
        id: content
        anchors.centerIn: parent
        spacing: 6
        RoundedImage {
            id: image
            visible: action.imageSource.toString().length > 0 && status === Image.Ready
            anchors.verticalCenter: parent.verticalCenter
            width: 14; height: 14
            radius: 3
            source: action.imageSource
            sourceSize: Qt.size(28, 28)
        }
        Icon {
            visible: action.iconPath.length > 0 && !image.visible
            anchors.verticalCenter: parent.verticalCenter
            width: 14; height: 14
            path: action.iconPath
            color: action.iconColor
        }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            text: action.text
            role: "bodySmall"
            color: action.selected ? Theme.accent
                : action.muted && !mouse.containsMouse ? Theme.surfaceContentVariant : Theme.surfaceContent
        }
        Txt {
            visible: action.badge.length > 0
            anchors.verticalCenter: parent.verticalCenter
            text: action.badge
            role: "bodySmall"
            color: Theme.surfaceContentVariant
        }
    }
    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: action.clicked()
    }
}
