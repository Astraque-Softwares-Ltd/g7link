// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick
import app.g7link

// When the Agents chat's rows move, as on the phone (its RowsMotion): only
// while what the reader just opened or closed settles. A click opens the
// window; the first rows that come for it keep it open just long enough for
// their motion to end, and if none come it closes after `wait`. Rows
// streaming in, the first load and Earlier's rows come while it's closed, so
// they never move. Nothing moves under reduced motion.
QtObject {
    id: motion

    // The chat's rows glide now.
    property bool gliding: false
    // A click is waiting for its rows.
    property bool waiting: false
    // From a click to its rows, at most.
    readonly property int wait: 1000
    // As the phone's: rows below slide, opened ones fade in a beat later,
    // closed ones fade out quickly.
    readonly property int moveMs: Theme.motionMs(240)
    readonly property int inMs: Theme.motionMs(180)
    readonly property int inDelay: Theme.motionMs(60)
    readonly property int outMs: Theme.motionMs(120)
    // A disclosure chevron turning.
    readonly property int turnMs: Theme.smallIn

    // Opens or closes `key` (as Agents.toggle), letting what moves glide.
    function toggle(key) {
        if (motion.moveMs > 0) {
            motion.gliding = true
            motion.waiting = true
            motion.settle.interval = motion.wait
            motion.settle.restart()
        }
        Agents.toggle(key)
    }

    // Rows came or went while a click waited for them: once, they settle.
    function landed() {
        if (!motion.waiting)
            return
        motion.waiting = false
        motion.settle.interval = motion.moveMs + motion.inDelay + 120
        motion.settle.restart()
    }

    property Timer settle: Timer {
        onTriggered: {
            motion.waiting = false
            motion.gliding = false
        }
    }
}
