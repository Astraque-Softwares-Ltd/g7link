// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A small pill with optional icon. `selected` uses the indicator colors.
Rectangle {
    id: chip
    property string text
    property string iconPath
    property bool selected: false
    property bool interactive: false
    signal clicked()

    implicitHeight: Theme.quiet ? 24 : 30
    implicitWidth: row.implicitWidth + (Theme.quiet ? 20 : 24)
    radius: Theme.quiet ? Theme.radiusSm : Theme.pill(height)
    color: Theme.quiet
        ? (selected ? Theme.activeFill : (interactive ? "transparent" : Theme.raised))
        : Theme.graphite
        ? (selected ? Theme.surfaceContent : "transparent")
        : (Theme.glass
            ? (selected
                ? Qt.rgba(Theme.primary.r, Theme.primary.g, Theme.primary.b, Theme.dark ? 0.28 : 0.22)
                : Qt.rgba(1, 1, 1, Theme.dark ? 0.10 : 0.50))
            : (selected ? Theme.secondaryContainer : Theme.surfaceContainerHigh))
    Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
    border.width: Theme.quiet ? (selected ? 1 : 0) : (Theme.graphite || Theme.glass ? 1 : 0)
    border.color: Theme.quiet ? Theme.accent : Theme.glass
        ? (selected ? Qt.rgba(Theme.primary.r, Theme.primary.g, Theme.primary.b, 0.55) : Qt.rgba(1, 1, 1, Theme.dark ? 0.22 : 0.68))
        : (selected ? Theme.surfaceContent : Theme.outlineVariant)
    scale: chip.interactive && mouse.pressed && !Theme.reduceMotion ? Theme.pressScale : 1.0
    Behavior on scale { enabled: !Theme.reduceMotion; NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }

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
            width: Theme.quiet ? 14 : 15; height: width
            path: chip.iconPath
            color: label.color
        }
        Txt {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            text: chip.text
            role: Theme.quiet ? "bodySmall" : "label"
            color: Theme.quiet ? (chip.selected ? Theme.accent : !chip.interactive ? Theme.surfaceContent : Theme.surfaceContentVariant)
                : Theme.graphite ? (chip.selected ? Theme.surface : Theme.surfaceContent)
                                  : (chip.selected ? Theme.secondaryContainerContent : Theme.surfaceContent)
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
