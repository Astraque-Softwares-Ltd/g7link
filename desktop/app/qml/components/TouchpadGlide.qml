// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick

// A touchpad scroll in a Flickable follows the fingers and stops dead when
// they lift: Wayland sends no momentum and Qt Quick adds none. This lets the
// view glide on at the fingers' speed and slow down, as GNOME's own apps do.
// It only watches; the Flickable still does the scrolling under the fingers.
WheelHandler {
    id: glide

    required property Flickable view
    // The fingers' last moves, as [time in ms, pixels], oldest first.
    property var moves: []

    target: null
    blocking: false
    orientation: Qt.Vertical
    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad

    onWheel: (event) => {
        const recentMs = 100
        const now = Date.now()
        if (event.phase === Qt.ScrollUpdate) {
            glide.moves = glide.moves.filter((m) => now - m[0] <= recentMs).concat([[now, event.pixelDelta.y]])
        } else if (event.phase === Qt.ScrollEnd) {
            // Fingers held still before lifting don't glide.
            const recent = glide.moves.filter((m) => now - m[0] <= recentMs)
            glide.moves = []
            if (recent.length < 2)
                return
            const span = Math.max(16, now - recent[0][0])
            const speed = recent.reduce((sum, m) => sum + m[1], 0) * 1000 / span
            // After the Flickable has ended its own scroll for this event.
            if (Math.abs(speed) >= 300)
                Qt.callLater(() => glide.view.flick(0, speed))
        }
    }
}
