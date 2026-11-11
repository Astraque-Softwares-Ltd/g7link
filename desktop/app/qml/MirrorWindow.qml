// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import QtQml
import QtQml.Models
import app.g7link

// A phone's screen, or one of its apps, in a window of its own, sized like
// the phone: the one place the phone is drawn large. One per mirroring
// (`mirrorKey`); closing it ends that one.
Window {
    id: win
    required property string mirrorKey
    // This window's mirroring (see Mirror.windows).
    readonly property var info: {
        try {
            return JSON.parse(Mirror.windows).find(w => w.key === mirrorKey) || ({})
        } catch (e) {
            return ({})
        }
    }
    readonly property string deviceName: info.name || ""
    readonly property string phase: info.phase || ""
    readonly property bool canControl: info.canControl === true

    // The window around the phone's screen: a thin header with the tools,
    // a bezel, and the phone's three keys under it (whole screen only).
    readonly property int headerHeight: 40
    readonly property int footerHeight: info.app ? 0 : 40
    readonly property int pad: 10
    readonly property int bezel: 6
    readonly property int chromeW: 2 * (pad + bezel)
    readonly property int chromeH: headerHeight + footerHeight + 2 * (pad + bezel)

    visible: true
    title: info.app ? qsTr("%1 · %2").arg(info.title).arg(deviceName)
         : deviceName.length > 0 ? qsTr("%1 · G7 Link").arg(deviceName) : qsTr("Phone screen · G7 Link")
    color: Theme.surface
    minimumWidth: 240
    minimumHeight: 240

    // A phone-shaped window the first time a picture arrives (portrait or
    // landscape), within the screen, or restored to where the user left it.
    readonly property size frame: video.frameSize

    function toggleFullScreen() {
        win.visibility = win.visibility === Window.FullScreen ? Window.AutomaticVisibility : Window.FullScreen
    }
    property bool sized: false
    property bool readyForResize: false

    Timer {
        id: resizeTimer
        interval: 300
        repeat: false
        onTriggered: {
            if (!win.info.app || !win.sized)
                return
            if (win.visibility !== Window.FullScreen)
                Mirror.saveGeometry(win.mirrorKey, win.x, win.y, win.width, win.height)
            if (win.phase === "showing")
                Mirror.resize(win.mirrorKey, video.width, video.height)
        }
    }

    Timer {
        id: moveTimer
        interval: 300
        repeat: false
        onTriggered: {
            if (win.info.app && win.sized && win.visibility !== Window.FullScreen)
                Mirror.saveGeometry(win.mirrorKey, win.x, win.y, win.width, win.height)
        }
    }

    onWidthChanged: if (info.app && sized && readyForResize) resizeTimer.restart()
    onHeightChanged: if (info.app && sized && readyForResize) resizeTimer.restart()
    onXChanged: if (info.app && sized && readyForResize) moveTimer.restart()
    onYChanged: if (info.app && sized && readyForResize) moveTimer.restart()
    onPhaseChanged: {
        if (phase === "showing" && info.app && sized)
            resizeTimer.restart()
        // Typing goes to the picture again, not to the hidden Try again button.
        if (phase === "showing")
            video.forceActiveFocus()
    }
    WindowPlacement {
        id: windowPlacement
    }
    function placeOnScreen(px, py, w, h) {
        const spot = windowPlacement.spot(px, py, w, h, Qt.application.screens, Screen)
        x = spot.x
        y = spot.y
    }

    onActiveChanged: if (win.active) Mirror.syncClipboardOnFocus(win.mirrorKey)

    onFrameChanged: {
        if (frame.width <= 0)
            return
        const availW = Screen.desktopAvailableWidth > 0 ? Screen.desktopAvailableWidth : 1920
        const availH = Screen.desktopAvailableHeight > 0 ? Screen.desktopAvailableHeight : 1080
        if (!sized && info.app && (info.savedWidth || 0) >= 240 && (info.savedHeight || 0) >= 240) {
            readyForResize = false
            const w = Math.max(240, Math.min(Math.round(availW * 0.95), info.savedWidth))
            const h = Math.max(240, Math.min(Math.round(availH * 0.95), info.savedHeight))
            width = w
            height = h
            if (info.savedX !== null && info.savedX !== undefined && info.savedY !== null && info.savedY !== undefined) {
                placeOnScreen(info.savedX, info.savedY, w, h)
            }
            sized = true
            Qt.callLater(() => { win.readyForResize = true })
            return
        }
        // An app that only runs one way up turned the phone's display (#126):
        // the window takes the app's shape, and its usual resize tells the
        // phone the matching size.
        if (sized && info.app && (frame.width > frame.height) !== (width - chromeW > height - chromeH)) {
            const turnedW = Math.max(240, Math.min(Math.round(availW * 0.95), height - chromeH + chromeW))
            const turnedH = Math.max(240, Math.min(Math.round(availH * 0.95), width - chromeW + chromeH))
            width = turnedW
            height = turnedH
            placeOnScreen(x, y, turnedW, turnedH)
            return
        }
        const maxH = availH * 0.85 - chromeH
        const maxW = availW * 0.85 - chromeW
        const scale = Math.min(maxH / frame.height, maxW / frame.width, 1)
        const screenRatio = (width - chromeW) / Math.max(1, height - chromeH)
        if (!sized || (!info.app && Math.abs(screenRatio - frame.width / frame.height) > 0.05)) {
            readyForResize = false
            width = Math.round(frame.width * scale) + chromeW
            height = Math.round(frame.height * scale) + chromeH
            sized = true
            if (info.app)
                Mirror.saveGeometry(mirrorKey, x, y, width, height)
            Qt.callLater(() => { win.readyForResize = true })
        }
    }
    width: 460
    height: 900
    onClosing: {
        if (info.app && sized)
            Mirror.saveGeometry(mirrorKey, x, y, width, height)
        Mirror.stop(mirrorKey)
    }

    // ---- The phone's screen: near-black, 18 px corners, the picture inside ----
    Rectangle {
        id: phoneScreen
        x: win.pad
        y: win.headerHeight + win.pad
        width: win.width - 2 * win.pad
        height: win.height - win.headerHeight - win.footerHeight - 2 * win.pad
        radius: 18
        color: Qt.darker(Theme.surface, 1.6)

        VideoView {
            id: video
            anchors.fill: parent
            anchors.margins: win.bezel
            stream: win.mirrorKey
            windowIcon: win.info.icon || ""
            focus: true
            acceptsText: win.phase === "showing" && win.canControl
            // The emoji picker and input methods give finished text here
            // instead of as key presses.
            onTextCommitted: (text) => Mirror.text(win.mirrorKey, text)

            // Where on the phone's screen a point in the window is (0 to 1).
            function at(x, y) {
                const r = pictureRect()
                return { x: (x - r.x) / Math.max(1, r.width), y: (y - r.y) / Math.max(1, r.height),
                         inside: x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height }
            }

            DropArea {
                id: mirrorDropArea
                anchors.fill: parent
                enabled: win.phase === "showing"
                onEntered: (drag) => {
                    Mirror.syncClipboardOnFocus(win.mirrorKey)
                }
                onDropped: (drop) => {
                    const urls = []
                    if (drop.urls) {
                        for (let i = 0; i < drop.urls.length; i++) {
                            urls.push(drop.urls[i].toString())
                        }
                    }
                    if (urls.length > 0) {
                        Mirror.syncClipboardOnFocus(win.mirrorKey)
                        Mirror.dropFiles(urls)
                    }
                    drop.acceptProposedAction()
                }
            }

            HoverHandler {
                id: videoHover
                onHoveredChanged: {
                    if (hovered) {
                        if (win.phase === "showing")
                            Mirror.enterWindow(win.mirrorKey)
                    } else {
                        Mirror.leaveWindow()
                    }
                }
            }

            // The mouse: left is a finger, right is Back, middle is Home.
            MouseArea {
                anchors.fill: parent
                enabled: win.phase === "showing"
                acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
                cursorShape: win.canControl ? Qt.PointingHandCursor : Qt.ArrowCursor
                property bool down: false
                property double lastMove: 0
                onPressed: (mouse) => {
                    video.forceActiveFocus()
                    if (!win.canControl) {
                        hintClose.closed = false
                        hintPulse.restart()
                        Mirror.requestControl(win.mirrorKey)
                        return
                    }
                    const p = video.at(mouse.x, mouse.y)
                    if (mouse.button === Qt.RightButton) { Mirror.press(win.mirrorKey, "back"); return }
                    if (mouse.button === Qt.MiddleButton) { Mirror.press(win.mirrorKey, "home"); return }
                    if (!p.inside) return
                    down = true
                    Mirror.touch(win.mirrorKey, "down", p.x, p.y)
                }
                onPositionChanged: (mouse) => {
                    if (!down || !win.canControl) return
                    // About 60 a second is plenty for a finger.
                    const now = Date.now()
                    if (now - lastMove < 16) return
                    lastMove = now
                    const p = video.at(mouse.x, mouse.y)
                    Mirror.touch(win.mirrorKey, "move", p.x, p.y)
                }
                onReleased: (mouse) => {
                    if (!down) return
                    down = false
                    if (!win.canControl) return
                    const p = video.at(mouse.x, mouse.y)
                    Mirror.touch(win.mirrorKey, "up", p.x, p.y)
                }
                onWheel: (wheel) => {
                    if (!win.canControl) {
                        hintClose.closed = false
                        Mirror.requestControl(win.mirrorKey)
                        return
                    }
                    const p = video.at(wheel.x, wheel.y)
                    if (p.inside)
                        Mirror.scroll(win.mirrorKey, p.x, p.y, -wheel.angleDelta.x / 120, -wheel.angleDelta.y / 120)
                }
            }

            // The keyboard: shortcuts for screenshots and recording work even
            // without remote control; text and navigation keys go to the phone.
            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_F11) {
                    win.toggleFullScreen()
                    event.accepted = true
                    return
                }
                if (win.phase !== "showing")
                    return
                if ((event.modifiers & Qt.ControlModifier) && (event.modifiers & Qt.ShiftModifier) && event.key === Qt.Key_C) {
                    Mirror.screenshotClipboard(win.mirrorKey)
                    event.accepted = true
                    return
                }
                if ((event.modifiers & Qt.ControlModifier) && !(event.modifiers & Qt.ShiftModifier) && event.key === Qt.Key_S) {
                    Mirror.screenshotFile(win.mirrorKey)
                    event.accepted = true
                    return
                }
                if ((event.modifiers & Qt.ControlModifier) && !(event.modifiers & Qt.ShiftModifier) && event.key === Qt.Key_R) {
                    Mirror.toggleRecording(win.mirrorKey)
                    event.accepted = true
                    return
                }
                if (!win.canControl)
                    return
                if (Preferences.enterSends && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)) {
                    // A plain Enter presses the app's Send; with a modifier, a new line.
                    if (event.modifiers & (Qt.ControlModifier | Qt.ShiftModifier))
                        Mirror.text(win.mirrorKey, "\n")
                    else
                        Mirror.press(win.mirrorKey, "send")
                    event.accepted = true
                    return
                }
                const keys = {}
                keys[Qt.Key_Escape] = "back"
                keys[Qt.Key_Back] = "back"
                keys[Qt.Key_Home] = "home"
                keys[Qt.Key_Return] = "enter"
                keys[Qt.Key_Enter] = "enter"
                keys[Qt.Key_Backspace] = "backspace"
                keys[Qt.Key_Delete] = "delete"
                keys[Qt.Key_Left] = "left"
                keys[Qt.Key_Right] = "right"
                keys[Qt.Key_Up] = "up"
                keys[Qt.Key_Down] = "down"
                keys[Qt.Key_Tab] = "tab"
                // Copy, cut and select all on the phone; what's copied there
                // comes back with the clipboard.
                const edits = {}
                edits[Qt.Key_C] = "copy"
                edits[Qt.Key_X] = "cut"
                edits[Qt.Key_A] = "select_all"
                const ctrlOnly = (event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.ShiftModifier | Qt.MetaModifier))
                    === Qt.ControlModifier
                if ((event.modifiers & Qt.ControlModifier) && event.key === Qt.Key_V) {
                    Mirror.paste(win.mirrorKey)
                    event.accepted = true
                } else if (ctrlOnly && edits[event.key] !== undefined) {
                    Mirror.press(win.mirrorKey, edits[event.key])
                    event.accepted = true
                } else if (keys[event.key] !== undefined) {
                    Mirror.press(win.mirrorKey, keys[event.key])
                    event.accepted = true
                } else if (event.text.length > 0 && event.text.charCodeAt(0) >= 32
                           && (!(event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier))
                               // AltGr (@ on a French keyboard, say) can arrive as Ctrl+Alt.
                               // A plain letter, digit or space with Ctrl+Alt is a shortcut:
                               // Qt gives Ctrl+Alt+2 the text "2" even without AltGr.
                               || ((event.modifiers & Qt.ControlModifier) && (event.modifiers & Qt.AltModifier)
                                   && !/^[A-Za-z0-9 ]$/.test(event.text)))) {
                    Mirror.text(win.mirrorKey, event.text)
                    event.accepted = true
                }
            }
        }

        // Brief confirmation pill for screenshots, recordings, file drops, and power toggles.
        Rectangle {
            id: noticePill
            readonly property string pillText: mirrorDropArea.containsDrag
                ? qsTr("Release to send files to %1").arg(win.deviceName.length > 0 ? win.deviceName : qsTr("phone"))
                : (win.info.notice || "")
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: hintBanner.visible ? hintBanner.top : parent.bottom
            anchors.bottomMargin: hintBanner.visible ? 8 : 16
            width: Math.min(parent.width - 24, noticeTxt.implicitWidth + 24)
            height: noticeTxt.implicitHeight + 12
            radius: Theme.radiusMd
            color: Theme.raised
            border.width: 1
            border.color: Theme.line
            visible: opacity > 0
            opacity: pillText.length > 0 ? 1 : 0
            Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
            Txt {
                id: noticeTxt
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                role: "bodySmall"
                text: noticePill.pillText
            }
        }

        // How to control it, until the phone allows it.
        Rectangle {
            id: hintBanner
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: 12
            height: Math.max(hint.implicitHeight + 20, 48)
            radius: Theme.radiusMd
            visible: win.phase === "showing" && video.frameSize.width > 0 && !win.canControl && !win.info.app && !hintClose.closed
            color: Theme.surface
            border.width: 1
            border.color: Theme.line

            SequentialAnimation {
                id: hintPulse
                NumberAnimation {
                    target: hintBanner
                    property: "scale"
                    from: 1.0
                    to: Theme.reduceMotion ? 1.0 : 1.03
                    duration: 110
                    easing.type: Easing.OutQuad
                }
                NumberAnimation {
                    target: hintBanner
                    property: "scale"
                    from: Theme.reduceMotion ? 1.0 : 1.03
                    to: 1.0
                    duration: 140
                    easing.type: Easing.InOutQuad
                }
            }

            Txt {
                id: hint
                anchors.left: parent.left
                anchors.right: notifyPhoneBtn.left
                anchors.margins: 10
                anchors.verticalCenter: parent.verticalCenter
                wrapMode: Text.WordWrap
                role: "bodySmall"
                text: qsTr("To control it, turn on Control from your PC in G7 Link on the phone.")
            }
            Button {
                id: notifyPhoneBtn
                anchors.right: hintClose.left
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                variant: "tonal"
                size: "sm"
                text: qsTr("Notify phone")
                onClicked: Mirror.requestControl(win.mirrorKey)
            }
            IconButton {
                id: hintClose
                property bool closed: false
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                iconPath: Icons.close
                label: qsTr("Hide")
                onClicked: closed = true
            }
        }

        // Before the picture: waiting for the phone, or why it ended.
        Rectangle {
            anchors.fill: parent
            visible: win.phase !== "showing" || video.frameSize.width <= 0
            radius: phoneScreen.radius
            color: Theme.surface
            Column {
                anchors.centerIn: parent
                width: Math.min(parent.width - 48, 320)
                spacing: 14
                Spinner {
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: win.phase !== "ended"
                    width: 24; height: 24
                }
                RoundedImage {
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: win.phase === "ended" && win.info.app === true && (win.info.icon || "").length > 0
                    width: 40; height: 40
                    radius: 10
                    source: win.info.icon || ""
                }
                Icon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: win.phase === "ended" && !(win.info.app === true && (win.info.icon || "").length > 0)
                    width: 32; height: 32
                    path: Icons.mirror
                    color: Theme.surfaceContentVariant
                }
                Txt {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    weight: 500
                    text: win.phase === "asking" ? (win.info.app ? qsTr("Opening %1").arg(win.info.title) : qsTr("Allow it on your phone"))
                        : win.phase === "ended" ? (win.info.app ? qsTr("%1 closed").arg(win.info.title) : qsTr("Mirroring stopped"))
                        : qsTr("Starting…")
                }
                Txt {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    role: "bodySmall"
                    muted: true
                    text: win.phase === "asking" && !win.info.app
                          ? qsTr("Tap the notification on %1, then choose to share the entire screen.").arg(win.deviceName)
                          : win.phase === "ended" ? ((win.info.reason || "").length > 0 ? win.info.reason
                                                    : win.info.app ? qsTr("The app closed on %1.").arg(win.deviceName)
                                                    : qsTr("The phone stopped sharing its screen."))
                          : ""
                }
                Button {
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: win.phase === "ended"
                    variant: "tonal"
                    text: win.info.app ? qsTr("Open again") : qsTr("Try again")
                    onClicked: Mirror.reopen(win.mirrorKey)
                }
            }
        }
    }

    property int recordingElapsedSecs: 0
    function updateRecordingElapsed() {
        if (win.info.recording === true && (win.info.recordingStartedMs || 0) > 0) {
            win.recordingElapsedSecs = Math.max(0, Math.floor((Date.now() - win.info.recordingStartedMs) / 1000))
        } else {
            win.recordingElapsedSecs = 0
        }
    }
    onInfoChanged: updateRecordingElapsed()

    Timer {
        interval: 1000
        repeat: true
        running: win.info.recording === true
        onTriggered: win.updateRecordingElapsed()
    }

    function formatDuration(secs) {
        const m = Math.floor(secs / 60)
        const s = secs % 60
        return (m < 10 ? "0" + m : "" + m) + ":" + (s < 10 ? "0" + s : "" + s)
    }

    // Frames a second, from the picture's frame counter.
    property real fps: 0
    property real lastFrames: 0
    Timer {
        interval: 1000
        repeat: true
        running: win.phase === "showing" && win.visible
        onTriggered: {
            const now = video.frames
            win.fps = Math.max(0, now - win.lastFrames)
            win.lastFrames = now
        }
    }

    // The phone's link, for the header ("USB · 4 ms").
    Instantiator {
        id: links
        model: DeviceList
        delegate: QtObject {
            required property string path
            required property int rttMs
            required property bool online
        }
    }
    readonly property var link: {
        const row = links.count > 0 ? DeviceList.rowOf(info.device || "") : -1
        return row >= 0 ? links.objectAt(row) : null
    }
    readonly property string stats: {
        const parts = []
        if (win.phase === "showing" && win.fps > 0)
            parts.push(qsTr("%1 fps").arg(Math.round(win.fps)))
        if (win.link && win.link.online && win.link.rttMs >= 0)
            parts.push(qsTr("%1 ms").arg(win.link.rttMs))
        return parts.join(" · ")
    }

    function moreActions() {
        const list = [
            { label: qsTr("Copy screenshot"), icon: Icons.copy, tint: Theme.tintBlue, keys: "Ctrl Shift C",
              run: () => Mirror.screenshotClipboard(win.mirrorKey) },
            { label: qsTr("Save screenshot"), icon: Icons.camera, tint: Theme.tintBlue, keys: "Ctrl S",
              run: () => Mirror.screenshotFile(win.mirrorKey) },
            { label: win.info.recording === true ? qsTr("Stop recording") : qsTr("Record video"),
              icon: win.info.recording === true ? Icons.stopSquare : Icons.record, tint: Theme.tintRed, keys: "Ctrl R",
              run: () => Mirror.toggleRecording(win.mirrorKey) },
            { label: qsTr("Keep phone awake"), icon: Icons.coffee, tint: Theme.tintAmber, checked: win.info.stayAwake === true,
              run: () => Mirror.setStayAwake(win.mirrorKey, !(win.info.stayAwake === true)) }
        ]
        if (win.info.canScreenOff === true)
            list.push({ label: qsTr("Phone screen off"), icon: Icons.screenOff, tint: Theme.tintLavender,
                        checked: win.info.screenOff === true,
                        run: () => Mirror.setScreenOff(win.mirrorKey, !(win.info.screenOff === true)) })
        list.push({ label: qsTr("Lock phone"), icon: Icons.lock, tint: Theme.tintTeal, enabled: win.canControl,
                    run: () => Mirror.press(win.mirrorKey, "lock") })
        if (!win.canControl && !win.info.app)
            list.push({ label: qsTr("Ask the phone for control"), icon: Icons.bell, tint: Theme.tintBlue,
                        run: () => Mirror.requestControl(win.mirrorKey) })
        list.push({ label: win.visibility === Window.FullScreen ? qsTr("Leave full screen") : qsTr("Full screen"),
                    icon: win.visibility === Window.FullScreen ? Icons.fullscreenExit : Icons.fullscreen,
                    keys: "F11", run: () => win.toggleFullScreen() })
        return list
    }

    // ---- Header: the phone, its frame rate and ping, and the tools ----
    Item {
        id: header
        width: parent.width
        height: win.headerHeight

        Row {
            x: 12
            anchors.verticalCenter: parent.verticalCenter
            width: Math.max(0, tools.x - x - 8)
            spacing: 8
            clip: true
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, parent.width - (statsText.visible ? statsText.implicitWidth + 8 : 0))
                text: win.info.app ? (win.info.title || "") : (win.deviceName.length > 0 ? win.deviceName : qsTr("Phone"))
                role: "bodySmall"
                weight: 500
            }
            Txt {
                id: statsText
                anchors.verticalCenter: parent.verticalCenter
                visible: text.length > 0
                text: win.info.app && win.deviceName.length > 0
                    ? (win.stats.length > 0 ? qsTr("%1 · %2").arg(win.deviceName).arg(win.stats) : win.deviceName)
                    : win.stats
                role: "bodySmall"
                muted: true
            }
        }

        Row {
            id: tools
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            spacing: 0
            visible: win.phase === "showing" && video.frameSize.width > 0

            IconButton {
                id: soundBtn
                // Muted, no sound comes in: this brings it back.
                visible: win.info.sound === true || (Mirror.muted && !win.info.app)
                iconPath: Mirror.muted ? Icons.soundOff : Icons.speaker
                iconSize: 16
                iconColor: Mirror.muted ? Theme.surfaceContentVariant : Theme.tintTeal
                label: Mirror.muted ? qsTr("Play the phone's sound here") : qsTr("Play the sound on the phone")
                onClicked: Mirror.toggleSound()
            }
            IconButton {
                iconPath: Icons.camera
                iconSize: 16
                iconColor: Theme.tintBlue
                label: qsTr("Screenshot (Ctrl+S)")
                onClicked: Mirror.screenshotFile(win.mirrorKey)
            }
            IconButton {
                iconPath: win.info.recording === true ? Icons.stopSquare : Icons.record
                iconSize: 16
                iconColor: Theme.tintRed
                label: win.info.recording === true ? qsTr("Stop recording (Ctrl+R)") : qsTr("Record (Ctrl+R)")
                onClicked: Mirror.toggleRecording(win.mirrorKey)
            }
            Txt {
                visible: win.info.recording === true
                anchors.verticalCenter: parent.verticalCenter
                rightPadding: 6
                text: win.formatDuration(win.recordingElapsedSecs)
                role: "bodySmall"
                color: Theme.tintRed
            }
            IconButton {
                iconPath: win.visibility === Window.FullScreen ? Icons.fullscreenExit : Icons.fullscreen
                iconSize: 16
                label: win.visibility === Window.FullScreen ? qsTr("Leave full screen (F11)") : qsTr("Full screen (F11)")
                onClicked: win.toggleFullScreen()
            }
            IconButton {
                id: moreBtn
                iconPath: Icons.more
                iconSize: 16
                label: qsTr("More")
                onClicked: {
                    moreMenu.actions = win.moreActions()
                    moreMenu.openAt(win.width - 8, header.height - 4, false, true)
                }
            }
        }

        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.line
        }
    }

    // ---- The phone's three keys, whole screen only ----
    Row {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Math.round((win.footerHeight - 30) / 2) + 2
        visible: !win.info.app
        spacing: 40
        // They need control from this PC, like the picture itself.
        enabled: win.phase === "showing" && win.canControl

        IconButton {
            iconPath: Icons.back
            iconSize: 16
            label: qsTr("Back (right-click)")
            onClicked: Mirror.press(win.mirrorKey, "back")
        }
        IconButton {
            iconPath: Icons.home
            iconSize: 16
            label: qsTr("Home (middle-click)")
            onClicked: Mirror.press(win.mirrorKey, "home")
        }
        IconButton {
            iconPath: Icons.apps
            iconSize: 16
            label: qsTr("Recent apps")
            onClicked: Mirror.press(win.mirrorKey, "recents")
        }
    }

    PopupMenu {
        id: moreMenu
        panelWidth: 280
        onOpenedChanged: if (!opened) video.forceActiveFocus()
    }
}
