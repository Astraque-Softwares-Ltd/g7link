// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A destination in the G7 theme's bottom dock: an icon beside its name. On narrow
// windows only the selected one keeps its name on screen.
FocusScope {
    id: item
    property string iconPath
    property string text
    property bool selected: false
    property bool showLabel: true
    signal clicked

    implicitWidth: row.implicitWidth + 22
    implicitHeight: 32
    activeFocusOnTab: true

    Accessible.role: Accessible.PageTab
    Accessible.name: text
    Accessible.selected: selected
    Accessible.onPressAction: item.clicked()
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
            item.clicked()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: Theme.radiusMd
        color: Theme.indicator
        opacity: item.selected || mouse.containsMouse ? 1 : 0
        scale: mouse.pressed ? Theme.pressScale : 1
        Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
    }
    Row {
        id: row
        anchors.centerIn: parent
        spacing: 7
        Icon {
            anchors.verticalCenter: parent.verticalCenter
            width: 16; height: 16
            path: item.iconPath
            color: item.selected ? Theme.surfaceContent : Theme.surfaceContentVariant
        }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            visible: item.showLabel || item.selected
            text: item.text
            role: "body"
            size: 13
            weight: 500
            color: item.selected ? Theme.surfaceContent : Theme.surfaceContentVariant
        }
    }
    Rectangle {
        anchors.fill: parent
        anchors.margins: -2
        radius: Theme.radiusMd + 2
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: Theme.focusVisible(item)
    }
    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: item.clicked()
    }
}
