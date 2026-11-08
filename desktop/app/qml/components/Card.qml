// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A content surface: filled in Bloom, honey-edged in Hive, frosted optical glass in Glass,
// hairline-bordered in Graphite, and no box at all in the G7 theme: just a line under it.
Rectangle {
    id: cardRoot
    property int padding: 16
    default property alias content: inner.data

    radius: Theme.radiusLg
    color: Theme.cardColor
    // Glass draws its own lit rim (GlassRim) instead of a flat hairline.
    border.width: (Theme.graphite || Theme.hive || Theme.ambientBlurActive || Theme.amoled) && !Theme.glass && !Theme.quiet ? 1 : 0
    border.color: Theme.cardBorder
    readonly property int inset: Theme.quiet ? Math.min(padding, 8) : padding
    implicitHeight: inner.childrenRect.height + inset * 2
    Behavior on color { ColorAnimation { duration: Theme.fadeNormal } }

    GlassRim {
        radius: cardRoot.radius
    }

    Item {
        id: inner
        z: 1
        anchors.fill: parent
        anchors.margins: parent.inset
    }

    Rectangle {
        visible: Theme.quiet
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 1
        color: Theme.outlineVariant
    }
}
