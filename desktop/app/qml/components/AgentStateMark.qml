// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A thread's or a session's state as a 12 px mark, by shape as well as
// colour (plans/agent-colour.md §2.9), so no two states rest on hue alone:
// a spinner working, a filled dot with a bang needing you, an x failed, a
// check done, a hollow ring paused or ended; a guessed "needs you" is a
// hollow amber ring.
Item {
    id: mark
    // "working", "waiting", "needs_you", "done", "idle", "failed", "interrupted", "ended",
    // "limited" (a session stopped at its usage limit: paused, a hollow ring).
    property string phase
    // A guess: the agent didn't say it waits.
    property bool guess: false

    readonly property color tone: AgentPalette.stateColor(phase)
    readonly property bool needs: (phase === "waiting" || phase === "needs_you") && !guess

    width: 12
    height: 12

    Spinner {
        visible: mark.phase === "working"
        anchors.fill: parent
        color: mark.tone
    }
    Rectangle {
        visible: mark.needs
        anchors.fill: parent
        radius: 6
        color: mark.tone
        Text {
            anchors.centerIn: parent
            text: "!"
            color: AgentPalette.onNeedsYou
            font.family: Theme.fontUi
            font.pixelSize: 10
            font.weight: 800
        }
    }
    Icon {
        visible: mark.phase === "failed"
        anchors.fill: parent
        path: Icons.close
        color: mark.tone
    }
    Icon {
        visible: mark.phase === "done" || mark.phase === "idle"
        anchors.fill: parent
        path: Icons.check
        color: mark.tone
    }
    // Paused, ended, or a guess: a hollow ring.
    Rectangle {
        visible: !mark.needs && ["working", "failed", "done", "idle"].indexOf(mark.phase) < 0
        anchors.centerIn: parent
        width: 9
        height: 9
        radius: 4.5
        color: "transparent"
        border.width: 1.5
        border.color: mark.guess ? AgentPalette.needsYou : AgentPalette.paused
    }
}
