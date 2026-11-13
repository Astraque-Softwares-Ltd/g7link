// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A row with a glyph, a title, an optional caption after it and a switch
// (Phone as speaker, Play on phone). `enabled: false` greys it out until it
// can be used.
Item {
    id: bar
    property string iconPath
    property color tint: Theme.surfaceContentVariant
    property string title
    // Shown after the title, muted, when not empty.
    property string caption
    property bool checked: false
    signal toggled(bool on)

    implicitWidth: 280
    height: Theme.rowHeight

    StateFill {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: Theme.raised
        on: rowMouse.containsMouse && bar.enabled
    }
    MouseArea {
        id: rowMouse
        anchors.fill: parent
        enabled: bar.enabled
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: bar.toggled(!bar.checked)
    }

    Icon {
        id: glyph
        x: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        width: 18; height: 18
        path: bar.iconPath
        color: bar.tint
        opacity: bar.enabled ? 1 : 0.38
    }
    Txt {
        id: titleText
        anchors.left: glyph.right
        anchors.leftMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        width: Math.min(implicitWidth, barSwitch.x - x - 10)
        text: bar.title
        opacity: bar.enabled ? 1 : 0.38
    }
    Txt {
        visible: bar.caption.length > 0
        anchors.left: titleText.right
        anchors.leftMargin: 8
        anchors.right: barSwitch.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        text: bar.caption
        role: "bodySmall"
        muted: true
    }
    Toggle {
        id: barSwitch
        anchors.right: parent.right
        anchors.rightMargin: Theme.rowPadding
        anchors.verticalCenter: parent.verticalCenter
        label: bar.title
        checked: bar.checked
        onToggled: (on) => bar.toggled(on)
    }
}
