// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A thin scroll indicator over the edge of a Flickable, in G7's style: it
// shows while the view scrolls or the pointer is over it, then fades. It can
// be dragged. It only appears when the content is longer (or, horizontal,
// wider) than the view. Place it over the view's edge:
//
//     G7ScrollBar { flickable: list; anchors.top: list.top; anchors.bottom: list.bottom; anchors.right: list.right }
Item {
    id: bar
    property Flickable flickable
    property int orientation: Qt.Vertical

    readonly property bool vertical: orientation === Qt.Vertical
    readonly property real ratio: !flickable ? 1
        : (vertical ? flickable.visibleArea.heightRatio : flickable.visibleArea.widthRatio)
    readonly property real position: !flickable ? 0
        : (vertical ? flickable.visibleArea.yPosition : flickable.visibleArea.xPosition)
    readonly property bool needed: ratio > 0 && ratio < 0.999
    readonly property bool moving: !!flickable && (vertical ? flickable.movingVertically : flickable.movingHorizontally)
    readonly property bool grabbed: grip.pressed
    // The pointer is over the view or the bar.
    readonly property bool pointerOver: viewHover.hovered || grip.containsMouse
    readonly property bool shown: needed && (moving || grabbed || pointerOver || recent.running)
    readonly property real length: vertical ? height : width
    readonly property real thumbLength: Math.max(24, Math.min(1, ratio) * length)
    readonly property real thumbAt: Math.max(0, Math.min(1 - Math.min(1, ratio), position)) / Math.max(0.0001, 1 - Math.min(1, ratio))
        * (length - thumbLength)

    implicitWidth: vertical ? 8 : 0
    implicitHeight: vertical ? 0 : 8
    width: vertical ? implicitWidth : (flickable ? flickable.width : 0)
    height: vertical ? (flickable ? flickable.height : 0) : implicitHeight
    visible: opacity > 0
    opacity: shown ? 1 : 0
    z: 10
    Behavior on opacity {
        enabled: !Theme.reduceMotion
        NumberAnimation { duration: bar.shown ? Theme.quickIn : Theme.smallOut }
    }

    // Wheel and touchpad scrolling don't set `moving`: the bar shows for a
    // moment after the position changes.
    onPositionChanged: if (needed) recent.restart()
    Timer {
        id: recent
        interval: 700
    }
    HoverHandler {
        id: viewHover
        parent: bar.flickable
    }

    Rectangle {
        id: thumb
        readonly property real thickness: grip.containsMouse || bar.grabbed ? 6 : 4
        x: bar.vertical ? bar.width - thickness - 1 : bar.thumbAt
        y: bar.vertical ? bar.thumbAt : bar.height - thickness - 1
        width: bar.vertical ? thickness : bar.thumbLength
        height: bar.vertical ? bar.thumbLength : thickness
        radius: thickness / 2
        color: Theme.surfaceContentVariant
        opacity: bar.grabbed ? 0.7 : 0.45
    }

    MouseArea {
        id: grip
        anchors.fill: parent
        enabled: bar.needed
        hoverEnabled: true
        preventStealing: true
        // Where on the thumb it was taken, so it doesn't jump.
        property real offset: 0
        function scrollTo(along) {
            const f = bar.flickable
            if (!f)
                return
            const travel = Math.max(1, bar.length - bar.thumbLength)
            const fraction = Math.max(0, Math.min(1, (along - offset) / travel))
            if (bar.vertical)
                f.contentY = f.originY + fraction * Math.max(0, f.contentHeight - f.height)
            else
                f.contentX = f.originX + fraction * Math.max(0, f.contentWidth - f.width)
        }
        onPressed: (mouse) => {
            const along = bar.vertical ? mouse.y : mouse.x
            const inThumb = along >= bar.thumbAt && along <= bar.thumbAt + bar.thumbLength
            offset = inThumb ? along - bar.thumbAt : bar.thumbLength / 2
            if (!inThumb)
                scrollTo(along)
        }
        onPositionChanged: (mouse) => {
            if (pressed)
                scrollTo(bar.vertical ? mouse.y : mouse.x)
        }
    }
}
