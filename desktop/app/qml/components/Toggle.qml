// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A switch. Click, Space or Enter flips it; `toggled` reports the
// user's choice (bind `checked` to the source of truth).
FocusScope {
    id: toggle
    property bool checked: false
    property string label
    signal toggled(bool checked)

    implicitWidth: 34
    implicitHeight: 20
    activeFocusOnTab: true
    opacity: enabled ? 1 : 0.38

    Accessible.role: Accessible.CheckBox
    Accessible.name: label
    Accessible.checkable: true
    Accessible.checked: checked
    Accessible.onToggleAction: toggle.toggled(!toggle.checked)
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            toggle.toggled(!toggle.checked)
            event.accepted = true
        }
    }

    Rectangle {
        id: track
        anchors.fill: parent
        radius: height / 2
        color: toggle.checked ? Theme.accent : Theme.line
        Behavior on color { ColorAnimation { duration: Theme.fadeFast } }

        Rectangle {
            id: thumb
            width: 14
            height: 14
            radius: height / 2
            anchors.verticalCenter: parent.verticalCenter
            x: toggle.checked ? parent.width - width - 3 : 3
            color: toggle.checked ? Theme.accentContent : Theme.surfaceContentVariant
            // Quick and settled, never springy: a switch must feel immediate.
            Behavior on x { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.reduceMotion ? 0 : 140; easing.type: Easing.OutCubic } }
            Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
        }
    }
    Rectangle {
        anchors.fill: track
        anchors.margins: -3
        radius: track.radius + 3
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: Theme.focusVisible(toggle)
    }
    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: toggle.toggled(!toggle.checked)
    }
}
