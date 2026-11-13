// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A button. Variants: "fill" (primary action), "tonal", "outline",
// "text". Sizes: "md" (34 px) and "sm" (26 px). Works with mouse, touch and
// keyboard (Tab to focus, Enter or Space to press).
FocusScope {
    id: button
    property string text
    property string iconPath
    property string variant: "fill"
    property string size: "md"
    property bool busy: false
    signal clicked

    readonly property bool small: size === "sm"
    readonly property bool interactive: enabled && !busy
    readonly property color background: variant === "fill" ? Theme.accent
        : (variant === "tonal" ? Theme.selection : (variant === "text" ? "transparent" : Theme.raised))
    readonly property color foreground: variant === "fill" ? Theme.accentContent
        : (variant === "text" ? Theme.surfaceContentVariant : Theme.surfaceContent)

    implicitHeight: small ? 26 : 34
    implicitWidth: row.implicitWidth + (small ? 20 : 28)
    activeFocusOnTab: true
    opacity: enabled ? 1 : 0.38

    Accessible.role: Accessible.Button
    Accessible.name: text
    Accessible.onPressAction: if (interactive) button.clicked()
    Keys.onPressed: (event) => {
        if (interactive && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
            button.clicked()
            event.accepted = true
        }
    }

    Rectangle {
        id: body
        width: parent.width
        height: parent.height
        radius: button.small ? Theme.radiusSm : Theme.radiusMd
        color: button.background
        Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
        scale: mouse.pressed && button.interactive ? Theme.pressScale : 1
        Behavior on scale { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.quickIn; easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeMove } }

        // Hover & Press state layer.
        Rectangle {
            anchors.fill: parent
            radius: parent.radius
            color: button.foreground
            opacity: !button.interactive ? 0 : (mouse.pressed ? 0.14 : (mouse.containsMouse ? 0.07 : 0))
            Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
        }

        Row {
            id: row
            anchors.centerIn: parent
            spacing: 8
            Spinner {
                visible: button.busy
                anchors.verticalCenter: parent.verticalCenter
                width: 16; height: 16
                color: button.foreground
            }
            Icon {
                visible: button.iconPath.length > 0 && !button.busy
                anchors.verticalCenter: parent.verticalCenter
                width: 16; height: width
                path: button.iconPath
                color: button.foreground
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: button.text
                role: "label"
                color: button.foreground
                size: button.small ? 12 : 13
                weight: button.variant !== "fill" ? 400 : Theme.typography.label.weight
                maximumLineCount: 1
                elide: Text.ElideRight
            }
        }
    }

    // Keyboard focus ring.
    Rectangle {
        anchors.fill: body
        anchors.margins: -3
        radius: body.radius + 3
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: Theme.focusVisible(button)
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        enabled: button.interactive
        hoverEnabled: true
        cursorShape: button.interactive ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: button.clicked()
    }
}
