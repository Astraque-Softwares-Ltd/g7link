// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import app.g7link

// What plays on a phone or this PC: art, title, artist and app, a seek bar
// and the transport. "More" adds ±10 s, volume and where the PC's sound
// plays. With both a phone and a PC playing, Phone / PC switches between them.
Item {
    id: card
    required property string deviceId
    required property string playerId
    required property string app
    required property string title
    required property string artist
    required property bool playing
    required property real duration
    required property real position
    required property real positionAt
    required property string art
    required property bool canPlay
    required property bool canPause
    required property bool canNext
    required property bool canPrevious
    required property bool canSeek

    property bool embeddedInHero: false
    property bool hasMultipleSources: false
    property string activeSource: deviceId === "local" ? "local" : "phone"
    property int phoneVolume: 65
    signal sourceSelected(string source)

    property bool expanded: false

    readonly property bool activelyRendered: card.visible && Qt.application.state === Qt.ApplicationActive

    implicitHeight: contentCol.implicitHeight
    height: implicitHeight

    // Twice a second while playing, so the bar and times advance.
    property real now: Date.now()
    Timer {
        interval: 500
        repeat: true
        running: card.activelyRendered && card.playing && card.duration > 0
        onTriggered: card.now = Date.now()
        onRunningChanged: card.now = Date.now()
    }
    onPositionAtChanged: now = Date.now()

    function switchSource(targetSource) {
        const currentSrc = card.isLocalPc ? "local" : "phone"
        if (!card.hasMultipleSources || targetSource === currentSrc)
            return
        card.sourceSelected(targetSource)
    }

    readonly property real elapsed: position < 0 ? 0
        : Math.min(duration > 0 ? duration : Infinity, position + (playing ? Math.max(0, now - positionAt) : 0))
    // While dragging, the bar shows where the user is pointing.
    property real dragTo: -1
    readonly property real shown: dragTo >= 0 ? dragTo : elapsed
    readonly property int shownSeconds: Math.floor(Math.max(0, card.shown) / 1000)
    readonly property int remainingSeconds: Math.floor(Math.max(0, card.duration - card.shown) / 1000)

    readonly property bool isLocalPc: card.deviceId === "local"
    property int dragVolume: -1
    readonly property int currentVolume: dragVolume >= 0
        ? dragVolume
        : (isLocalPc ? DeckController.volume : card.phoneVolume)
    readonly property bool currentMuted: isLocalPc ? DeckController.muted : (currentVolume <= 0)

    function send(action, to) { MediaList.command(deviceId, playerId, action, to === undefined ? 0 : to) }
    function skipBy(deltaMs) {
        if (!card.canSeek || card.duration <= 0) return
        const target = Math.round(Math.max(0, Math.min(card.duration, card.shown + deltaMs)))
        card.send("seek", target)
    }
    function clockFromSec(s) {
        const sec = Math.max(0, s)
        const h = Math.floor(sec / 3600)
        const m = Math.floor((sec % 3600) / 60)
        const ss = String(sec % 60).padStart(2, "0")
        return h > 0 ? h + ":" + String(m).padStart(2, "0") + ":" + ss : m + ":" + ss
    }

    // A sideways swipe or scroll switches between phone and PC.
    WheelHandler {
        enabled: card.hasMultipleSources
        acceptedDevices: PointerDevice.TouchPad | PointerDevice.Mouse
        onWheel: (event) => {
            const dx = event.angleDelta.x !== 0 ? event.angleDelta.x : ((event.modifiers & Qt.ShiftModifier) ? event.angleDelta.y : 0)
            if (dx <= -30) {
                card.switchSource("local")
                event.accepted = true
            } else if (dx >= 30) {
                card.switchSource("phone")
                event.accepted = true
            }
        }
    }

    Column {
        id: contentCol
        width: parent.width
        spacing: 10

        Row {
            visible: card.hasMultipleSources
            spacing: 4
            TextAction {
                text: qsTr("Phone")
                selected: !card.isLocalPc
                onClicked: card.switchSource("phone")
            }
            TextAction {
                text: qsTr("PC")
                selected: card.isLocalPc
                onClicked: card.switchSource("local")
            }
        }

        // ---- Art, title, artist · app ----
        Item {
            width: parent.width
            height: 40
            Rectangle {
                id: cover
                width: 40; height: 40
                radius: Theme.radiusSm
                color: Theme.selection
                clip: true
                Icon {
                    anchors.centerIn: parent
                    visible: coverImage.status !== Image.Ready
                    width: 18; height: 18
                    path: Icons.music
                    color: Theme.tintPink
                }
                RoundedImage {
                    id: coverImage
                    anchors.fill: parent
                    visible: status === Image.Ready
                    source: card.art
                    sourceSize: Qt.size(80, 80)
                }
            }
            Column {
                anchors.left: cover.right
                anchors.leftMargin: 10
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                Txt {
                    width: parent.width
                    text: card.title.length > 0 ? card.title : (card.app.length > 0 ? card.app : qsTr("Now playing"))
                }
                Txt {
                    width: parent.width
                    visible: text.length > 0
                    role: "bodySmall"
                    muted: true
                    text: [card.artist, card.isLocalPc ? qsTr("This PC") : card.app].filter(s => s && s.length > 0).join(" · ")
                }
            }
        }

        // ---- Seek bar ----
        Column {
            width: parent.width
            spacing: 4
            visible: card.duration > 0
            Item {
                id: bar
                width: parent.width
                height: 10
                activeFocusOnTab: card.canSeek
                Accessible.role: Accessible.Slider
                Accessible.name: qsTr("Playback position")
                Keys.onPressed: (event) => {
                    if (!card.canSeek || card.duration <= 0) return
                    if (event.key === Qt.Key_Left) {
                        card.skipBy(-5000)
                        event.accepted = true
                    } else if (event.key === Qt.Key_Right) {
                        card.skipBy(5000)
                        event.accepted = true
                    }
                }
                readonly property real fraction: card.duration > 0 ? Math.max(0, Math.min(1, card.shown / card.duration)) : 0
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width
                    height: 4
                    radius: 2
                    color: Theme.selection
                    border.width: Theme.focusVisible(bar) ? 1 : 0
                    border.color: Theme.focusRing
                    Rectangle {
                        width: parent.width * bar.fraction
                        height: parent.height
                        radius: parent.radius
                        color: Theme.tintPink
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -4
                    enabled: card.canSeek
                    cursorShape: card.canSeek ? Qt.PointingHandCursor : Qt.ArrowCursor
                    function at(x) { return Math.max(0, Math.min(1, (x - 4) / bar.width)) * card.duration }
                    onPressed: (mouse) => card.dragTo = at(mouse.x)
                    onPositionChanged: (mouse) => { if (pressed) card.dragTo = at(mouse.x) }
                    onReleased: (mouse) => {
                        card.send("seek", Math.round(at(mouse.x)))
                        card.dragTo = -1
                    }
                    onCanceled: card.dragTo = -1
                }
            }
            Item {
                width: parent.width
                height: elapsedText.height
                Txt { id: elapsedText; role: "caption"; muted: true; text: card.clockFromSec(card.shownSeconds) }
                Txt {
                    anchors.right: parent.right
                    role: "caption"
                    muted: true
                    text: "-" + card.clockFromSec(card.remainingSeconds)
                }
            }
        }

        // ---- Transport ----
        Item {
            width: parent.width
            height: 30
            Row {
                anchors.centerIn: parent
                spacing: 4
                IconButton {
                    visible: card.expanded && card.canSeek && card.duration > 0
                    iconPath: Icons.rewind10
                    label: qsTr("Rewind 10 seconds")
                    onClicked: card.skipBy(-10000)
                }
                IconButton {
                    enabled: card.canPrevious
                    iconPath: Icons.skipPrevious
                    iconColor: Theme.surfaceContent
                    label: qsTr("Previous")
                    onClicked: card.send("previous")
                }
                IconButton {
                    width: 46
                    tonal: true
                    enabled: card.playing ? card.canPause : card.canPlay
                    iconPath: card.playing ? Icons.pause : Icons.play
                    iconColor: Theme.surfaceContent
                    label: card.playing ? qsTr("Pause") : qsTr("Play")
                    onClicked: card.send(card.playing ? "pause" : "play")
                }
                IconButton {
                    enabled: card.canNext
                    iconPath: Icons.skipNext
                    iconColor: Theme.surfaceContent
                    label: qsTr("Next")
                    onClicked: card.send("next")
                }
                IconButton {
                    visible: card.expanded && card.canSeek && card.duration > 0
                    iconPath: Icons.forward10
                    label: qsTr("Forward 10 seconds")
                    onClicked: card.skipBy(10000)
                }
            }
            IconButton {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: 26; height: 26
                iconSize: 14
                iconPath: card.expanded ? Icons.chevronUp : Icons.chevronDown
                label: card.expanded ? qsTr("Fewer controls") : qsTr("More controls")
                onClicked: card.expanded = !card.expanded
            }
        }

        // ---- More: volume, where the sound plays, play on phone, colors ----
        Column {
            width: parent.width
            spacing: 6
            visible: card.expanded

            Item {
                width: parent.width
                height: 26
                IconButton {
                    id: muteBtn
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    width: 26; height: 26
                    iconSize: 14
                    iconPath: card.currentMuted ? Icons.soundOff : Icons.speaker
                    label: card.currentMuted ? qsTr("Unmute") : qsTr("Mute")
                    onClicked: card.send("mute", card.currentVolume)
                }
                Item {
                    id: volBar
                    anchors.left: muteBtn.right
                    anchors.leftMargin: 6
                    anchors.right: volText.left
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    height: 16
                    readonly property real volFraction: card.currentMuted ? 0 : Math.max(0, Math.min(1, card.currentVolume / 100))
                    Accessible.role: Accessible.Slider
                    Accessible.name: qsTr("Volume")
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: parent.width
                        height: 4
                        radius: 2
                        color: Theme.selection
                        Rectangle {
                            width: parent.width * volBar.volFraction
                            height: parent.height
                            radius: parent.radius
                            color: Theme.accent
                        }
                    }
                    Timer {
                        id: volSettleTimer
                        interval: 400
                        onTriggered: card.dragVolume = -1
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        function levelAt(x) { return Math.round(Math.max(0, Math.min(1, x / volBar.width)) * 100) }
                        onPressed: (mouse) => {
                            volSettleTimer.stop()
                            card.dragVolume = levelAt(mouse.x)
                        }
                        onPositionChanged: (mouse) => { if (pressed) card.dragVolume = levelAt(mouse.x) }
                        onReleased: (mouse) => {
                            const lvl = levelAt(mouse.x)
                            card.dragVolume = lvl
                            card.send("volume", lvl)
                            volSettleTimer.restart()
                        }
                        onCanceled: card.dragVolume = -1
                    }
                }
                Txt {
                    id: volText
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    width: 32
                    horizontalAlignment: Text.AlignRight
                    role: "bodySmall"
                    text: String(card.currentVolume)
                }
            }
            Flow {
                width: parent.width
                spacing: 4
                TextAction {
                    text: Preferences.streamPcAudioToPhone ? qsTr("Phone speaker") : qsTr("PC speaker")
                    iconPath: Preferences.streamPcAudioToPhone ? Icons.phone : Icons.speaker
                    iconColor: Theme.tintTeal
                    Accessible.description: qsTr("Where this PC's sound plays")
                    onClicked: Preferences.streamPcAudioToPhone = !Preferences.streamPcAudioToPhone
                }
                TextAction {
                    visible: card.isLocalPc && MediaList.playOnPhoneName !== ""
                    text: qsTr("Play on %1").arg(MediaList.playOnPhoneName)
                    iconPath: Icons.phone
                    iconColor: Theme.tintBlue
                    onClicked: card.send("play_on_phone")
                }
            }
        }
    }
}
