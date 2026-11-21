// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A quote chip in the composer (§SC9.1): lines of a diff, `path:first–last`.
// On send the lines go before the message in a fence under their path.
Rectangle {
    id: chip
    property string path
    property int first: 0
    property int last: 0
    signal removed

    readonly property string label: qsTr("%1:%2–%3").arg(path).arg(first).arg(last)

    width: Math.min(row.implicitWidth + 16, 320)
    height: 26
    radius: Theme.radiusSm
    color: AgentPalette.chipGround
    border.width: 1
    border.color: Theme.outlineVariant

    Row {
        id: row
        x: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 6
        Icon {
            anchors.verticalCenter: parent.verticalCenter
            width: 12
            height: 12
            path: Icons.text
            color: Theme.surfaceContentVariant
        }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, 240)
            text: chip.label
            role: "code"
            size: Theme.typography.caption.size
            elide: Text.ElideLeft
            maximumLineCount: 1
        }
        IconButton {
            anchors.verticalCenter: parent.verticalCenter
            width: 20
            height: 20
            iconPath: Icons.close
            iconSize: 11
            label: qsTr("Remove the quote")
            onClicked: chip.removed()
        }
    }
}
