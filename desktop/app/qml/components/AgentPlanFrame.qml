// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The sides of a plan's card behind one of its rows: the head rounds the
// top, the foot the bottom, a block between them is plain. The card is
// several rows of the timeline; together they read as one.
Item {
    id: frame
    // "head", "body" or "foot".
    property string part: "body"
    readonly property real r: Theme.radiusMd

    clip: true
    Rectangle {
        x: 0
        y: frame.part === "head" ? 0 : -frame.r - 1
        width: frame.width
        height: frame.height + (frame.part === "body" ? 2 * frame.r + 2 : frame.r + 1)
        radius: frame.r
        color: Theme.surfaceContainerLow
        border.width: 1
        border.color: Theme.outlineVariant
    }
}
