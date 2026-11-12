// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A round device or app badge: an icon, or the first letter of a name.
Rectangle {
    id: avatar
    property string name
    property string iconPath
    property bool emphasized: false
    // A color of its own (a contact's): one of the glyph tints, picked from
    // the hue, washed into the ground with the tint as ink.
    property real hue: -1
    readonly property bool tinted: hue >= 0
    readonly property var tintNames: ["teal", "blue", "coral", "pink", "green", "lavender", "amber"]
    readonly property color tintInk: Theme.tint(tinted ? tintNames[Math.floor(hue * 7) % 7]
                                                      : (emphasized ? "lavender" : "blue"))

    implicitWidth: 28
    implicitHeight: 28
    radius: width / 2
    color: tinted || emphasized ? Theme.wash(tintInk) : Theme.raised

    readonly property color ink: tinted || emphasized ? tintInk : Theme.surfaceContentVariant
    Icon {
        visible: avatar.iconPath.length > 0
        anchors.centerIn: parent
        width: avatar.width * 0.5; height: width
        path: avatar.iconPath
        color: avatar.ink
    }
    Txt {
        visible: avatar.iconPath.length === 0
        anchors.centerIn: parent
        text: avatar.name.length > 0 ? avatar.name.charAt(0).toUpperCase() : ""
        role: "label"
        weight: 500
        color: avatar.ink
    }
}
