// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The 6-digit pairing code, one digit per raised cell, as both screens
// show it. `dim` shows placeholders before there's a code.
Row {
    id: digits
    property string code
    property bool dim: false
    property int cellWidth: 30
    property int cellHeight: 38
    property int pixelSize: 17
    spacing: 6

    Accessible.role: Accessible.StaticText
    Accessible.name: dim ? "" : code.split("").join(" ")

    Repeater {
        model: digits.code.split("")
        delegate: Rectangle {
            required property string modelData
            width: digits.cellWidth; height: digits.cellHeight
            radius: Theme.radiusSm
            color: Theme.raised
            Txt {
                anchors.centerIn: parent
                text: parent.modelData
                role: "code"
                size: digits.pixelSize
                muted: digits.dim
                font.letterSpacing: 0
            }
        }
    }
}
