// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick

// A touchpad scroll in a Flickable follows the fingers and stops dead when
// they lift: Wayland sends no momentum and Qt Quick adds none. This lets the
// view glide on at the speed it was moving and slow down, as GNOME's own apps
// do. It only watches; the Flickable still does the scrolling under the
// fingers. Declare it directly inside the view, so it sits on the view itself.
WheelHandler {
    id: glide

    required property Flickable view
    // Where the view was lately while the fingers moved, as [time in ms,
    // contentY], oldest first.
    property var trail: []

    target: null
    blocking: false
    orientation: Qt.Vertical
    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad

    onWheel: (event) => {
        const recentMs = 100
        const now = Date.now()
        if (event.phase === Qt.ScrollUpdate) {
            // Taken before the view handles this event: where the last one left it.
            glide.trail = glide.trail.filter((p) => now - p[0] <= recentMs).concat([[now, glide.view.contentY]])
        } else if (event.phase === Qt.ScrollEnd) {
            // Fingers held still before lifting don't glide, and nor does a view
            // that didn't move: at its end, or around another that scrolled instead.
            const recent = glide.trail.filter((p) => now - p[0] <= recentMs)
            glide.trail = []
            if (recent.length < 2)
                return
            const span = Math.max(16, now - recent[0][0])
            const speed = (recent[0][1] - glide.view.contentY) * 1000 / span
            // After the view has ended its own scroll for this event.
            if (Math.abs(speed) >= 300)
                Qt.callLater(() => glide.view.flick(0, speed))
        }
    }
}
