// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A small tag with optional icon. `selected` takes the accent.
Rectangle {
    id: chip
    property string text
    property string iconPath
    property bool selected: false
    property bool interactive: false
    signal clicked()

    implicitHeight: 24
    implicitWidth: row.implicitWidth + 20
    radius: Theme.radiusSm
    color: selected ? Theme.activeFill : (interactive ? "transparent" : Theme.raised)
    Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
    border.width: selected ? 1 : 0
    border.color: Theme.accent
    scale: chip.interactive && mouse.pressed && !Theme.reduceMotion ? Theme.pressScale : 1.0
    Behavior on scale { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.quickIn; easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeMove } }

    activeFocusOnTab: interactive && visible && enabled
    Accessible.role: interactive ? Accessible.Button : Accessible.StaticText
    Accessible.name: chip.text
    Accessible.onPressAction: if (chip.interactive) chip.clicked()
    Keys.onPressed: (event) => {
        if (chip.interactive && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
            chip.clicked()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: chip.radius
        color: label.color
        opacity: !chip.interactive ? 0 : (mouse.pressed ? 0.14 : (mouse.containsMouse ? 0.07 : 0))
        Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: -3
        radius: chip.radius + 3
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: chip.interactive && Theme.focusVisible(chip)
    }

    Row {
        id: row
        anchors.centerIn: parent
        spacing: 6
        Icon {
            visible: chip.iconPath.length > 0
            anchors.verticalCenter: parent.verticalCenter
            width: 14; height: width
            path: chip.iconPath
            color: label.color
        }
        Txt {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            text: chip.text
            role: "bodySmall"
            color: chip.selected ? Theme.accent : !chip.interactive ? Theme.surfaceContent : Theme.surfaceContentVariant
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        enabled: chip.interactive
        hoverEnabled: chip.interactive
        cursorShape: chip.interactive ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: chip.clicked()
    }
}
