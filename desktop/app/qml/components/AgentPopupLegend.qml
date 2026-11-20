// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The keys under the `@` and `/` lists: choose, insert, close.
Row {
    spacing: 6
    Keycap {
        anchors.verticalCenter: parent.verticalCenter
        keys: "↑↓"
    }
    Txt {
        anchors.verticalCenter: parent.verticalCenter
        text: qsTr("choose")
        role: "caption"
        muted: true
    }
    Item {
        width: 6
        height: 1
    }
    Keycap {
        anchors.verticalCenter: parent.verticalCenter
        keys: "↵"
    }
    Txt {
        anchors.verticalCenter: parent.verticalCenter
        text: qsTr("insert")
        role: "caption"
        muted: true
    }
    Item {
        width: 6
        height: 1
    }
    Keycap {
        anchors.verticalCenter: parent.verticalCenter
        keys: "Esc"
    }
    Txt {
        anchors.verticalCenter: parent.verticalCenter
        text: qsTr("close")
        role: "caption"
        muted: true
    }
}
