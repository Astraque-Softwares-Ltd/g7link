// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Connection indicator: a teal dot when online.
Item {
    property bool online: false
    implicitWidth: 13
    implicitHeight: 13

    Rectangle {
        anchors.centerIn: parent
        width: 6; height: width; radius: width / 2
        color: parent.online ? Theme.tintTeal : Theme.outline
        Behavior on color { ColorAnimation { duration: Theme.fadeNormal } }
    }
}
