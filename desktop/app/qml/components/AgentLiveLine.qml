// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// What the agent is doing between steps: "Starting…" or "Thinking", with a
// band of light sweeping over it, how long the turn has run, and the
// thought's first line when the agent shares it.
Item {
    id: live
    required property string liveLabel
    required property string text
    required property real sinceMs
    // Date.now(), ticking.
    property real now: Date.now()
    // The page is on screen: the band may move.
    property bool animate: true

    readonly property string label: liveLabel === "starting" ? qsTr("Starting…") : qsTr("Thinking")
    // "1:48", as the lists show a running time.
    function clock(ms) {
        const total = Math.max(0, Math.floor((now - ms) / 1000))
        const h = Math.floor(total / 3600)
        const m = Math.floor((total % 3600) / 60)
        const s = String(total % 60).padStart(2, "0")
        return h > 0 ? h + ":" + String(m).padStart(2, "0") + ":" + s : m + ":" + s
    }

    width: ListView.view ? ListView.view.width : 0
    height: column.height + 12

    Accessible.role: Accessible.StaticText
    Accessible.name: text.length > 0 ? label + ". " + text : label

    Icon {
        x: 20
        y: 8
        width: 16
        height: 16
        path: Icons.lightbulb
        color: Theme.surfaceContentVariant
    }
    Column {
        id: column
        x: 44
        y: 6
        width: parent.width - x - clockText.width - 8
        spacing: 2
        Item {
            width: word.implicitWidth
            height: word.height
            Txt {
                id: word
                text: live.label
                role: "bodySmall"
                muted: true
            }
            // The band: the word in full colour, seen through a window that
            // runs across it.
            Item {
                id: band
                width: 36
                height: parent.height
                visible: sweep.running
                clip: true
                Txt {
                    x: -band.x
                    text: live.label
                    role: "bodySmall"
                }
                NumberAnimation on x {
                    id: sweep
                    from: -band.width
                    to: word.implicitWidth
                    duration: 1400
                    loops: Animation.Infinite
                    running: live.animate && !Theme.reduceMotion && live.visible
                }
            }
        }
        Txt {
            visible: live.text.length > 0
            width: parent.width
            text: live.text
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            maximumLineCount: 3
        }
    }
    Txt {
        id: clockText
        visible: live.sinceMs > 0
        anchors.right: parent.right
        y: 6
        text: live.clock(live.sinceMs)
        role: "bodySmall"
        muted: true
    }
}
