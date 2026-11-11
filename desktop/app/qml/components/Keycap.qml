// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A key or chord, as the G7 launcher shows them: a flat fill, 5 px corners,
// muted text, no border. Key names ("Ctrl K", "Enter") aren't translated.
Rectangle {
    id: cap
    property string keys

    visible: keys.length > 0
    implicitWidth: label.implicitWidth + 12
    implicitHeight: 18
    radius: 5
    color: Theme.selection

    Txt {
        id: label
        anchors.centerIn: parent
        text: cap.keys
        role: "caption"
        muted: true
    }
}
