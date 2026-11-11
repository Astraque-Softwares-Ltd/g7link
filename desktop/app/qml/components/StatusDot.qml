// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Connection indicator: a dot with a soft halo when online (the theme's
// primary carries "connected"; the G7 theme uses its teal, without halo).
Item {
    property bool online: false
    implicitWidth: 13
    implicitHeight: 13

    Rectangle {
        anchors.centerIn: parent
        width: 13; height: 13; radius: 6.5
        color: Theme.success
        opacity: parent.online && !Theme.quiet ? 0.22 : 0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeNormal } }
    }
    Rectangle {
        anchors.centerIn: parent
        width: Theme.quiet ? 6 : 7; height: width; radius: width / 2
        color: parent.online ? (Theme.quiet ? Theme.tintTeal : Theme.success) : Theme.outline
        Behavior on color { ColorAnimation { duration: Theme.fadeNormal } }
    }
}
