// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import QtQml
import QtQml.Models
import app.g7link

// A phone's screen, or one of its apps, in a window of its own: the window
// is the picture and nothing else, in the picture's shape. Its tools wait
// behind one small button that shows while the pointer moves over it. One
// per mirroring (`mirrorKey`); closing it ends that one.
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
    readonly property bool showing: phase === "showing" && video.frameSize.width > 0

    visible: true
    title: info.app ? qsTr("%1 · %2").arg(info.title).arg(deviceName)
         : deviceName.length > 0 ? qsTr("%1 · G7 Link").arg(deviceName) : qsTr("Phone screen · G7 Link")
    // What shows around the picture while the window has another shape
    // (full screen, maximized, or between a resize and the phone's answer).
    color: "black"
    minimumWidth: 220
    minimumHeight: 220

    readonly property size frame: video.frameSize
    // Full screen and maximized windows keep the size the desktop gave them.
    readonly property bool ownSize: visibility === Window.Windowed

    function toggleFullScreen() {
        win.visibility = win.visibility === Window.FullScreen ? Window.AutomaticVisibility : Window.FullScreen
    }
    // The window took its first size from a picture (or from where it was left).
    property bool sized: false
    property bool readyForResize: false
    // The size the window last gave itself, to tell which side the user dragged.
    property int shapedWidth: 0
    property int shapedHeight: 0

    Timer {
        id: resizeTimer
        interval: 300
        repeat: false
        onTriggered: {
            if (!win.info.app || !win.sized)
                return
            if (win.ownSize)
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
            if (win.info.app && win.sized && win.ownSize)
                Mirror.saveGeometry(win.mirrorKey, win.x, win.y, win.width, win.height)
        }
    }

    // The window takes the picture's shape once both have settled: after the
    // user stops dragging an edge, and after the phone answered a new size.
    Timer {
        id: shapeTimer
        interval: 450
        repeat: false
        onTriggered: win.takeShape()
    }

    function availableSize() {
        return Qt.size(Screen.desktopAvailableWidth > 0 ? Screen.desktopAvailableWidth : 1920,
                       Screen.desktopAvailableHeight > 0 ? Screen.desktopAvailableHeight : 1080)
    }

    function setSize(w, h) {
        readyForResize = false
        width = w
        height = h
        shapedWidth = w
        shapedHeight = h
        Qt.callLater(() => { win.readyForResize = true })
    }

    // Gives the window the picture's shape, keeping the side the user dragged
    // (or the long side when the picture turned), within the screen.
    function takeShape() {
        if (!sized || !ownSize || frame.width <= 0 || frame.height <= 0)
            return
        const ratio = frame.width / frame.height
        if (Math.abs(width / Math.max(1, height) - ratio) < 0.01)
            return
        const avail = availableSize()
        const maxW = Math.round(avail.width * 0.95)
        const maxH = Math.round(avail.height * 0.95)
        let w = width
        let h = height
        if ((frame.width > frame.height) !== (width > height)) {
            const longSide = Math.max(width, height)
            w = frame.width > frame.height ? longSide : longSide * ratio
            h = frame.width > frame.height ? longSide / ratio : longSide
        } else if (Math.abs(height - shapedHeight) > Math.abs(width - shapedWidth)) {
            w = height * ratio
        } else {
            h = width / ratio
        }
        const fit = Math.min(1, maxW / w, maxH / h)
        const grow = Math.max(1, minimumWidth / (w * fit), minimumHeight / (h * fit))
        setSize(Math.round(w * fit * grow), Math.round(h * fit * grow))
        if (info.app) {
            Mirror.saveGeometry(mirrorKey, x, y, width, height)
            resizeTimer.restart()
        }
    }

    function shapeSoon(ms) {
        shapeTimer.interval = ms
        shapeTimer.restart()
    }

    function noteResize() {
        if (!sized || !readyForResize)
            return
        shapeSoon(450)
        if (info.app)
            resizeTimer.restart()
    }

    onWidthChanged: noteResize()
    onHeightChanged: noteResize()
    onXChanged: if (info.app && sized && readyForResize) moveTimer.restart()
    onYChanged: if (info.app && sized && readyForResize) moveTimer.restart()
    onVisibilityChanged: if (ownSize) shapeSoon(450)
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

    // Asked for again while it's open (its notification was clicked, or its
    // name was picked in the app list): comes to the front instead of opening twice.
    readonly property int raised: info.raised || 0
    onRaisedChanged: {
        if (raised <= 0)
            return
        if (visibility === Window.Minimized)
            showNormal()
        raise()
        requestActivate()
    }

    // A new window comes in front of the one it was opened from: GNOME puts
    // a window that only shows itself behind it. Asked once it's on screen
    // (its first frame), when the desktop has a window to bring forward.
    property bool broughtForward: false
    Connections {
        target: win
        enabled: !win.broughtForward
        function onFrameSwapped() {
            win.broughtForward = true
            win.raise()
            win.requestActivate()
        }
    }

    onActiveChanged: if (win.active) Mirror.syncClipboardOnFocus(win.mirrorKey)

    onFrameChanged: {
        if (frame.width <= 0 || frame.height <= 0)
            return
        if (sized) {
            shapeSoon(450)
            return
        }
        const avail = availableSize()
        if (info.app && (info.savedWidth || 0) >= minimumWidth && (info.savedHeight || 0) >= minimumHeight) {
            // Where the user left this app's window; the phone is told that
            // size, and the window follows the shape it answers with.
            const w = Math.min(Math.round(avail.width * 0.95), info.savedWidth)
            const h = Math.min(Math.round(avail.height * 0.95), info.savedHeight)
            setSize(w, h)
            if (info.savedX !== null && info.savedX !== undefined && info.savedY !== null && info.savedY !== undefined)
                placeOnScreen(info.savedX, info.savedY, w, h)
            sized = true
            // Long enough for the phone to answer that size with a picture.
            shapeSoon(1500)
            return
        }
        // A phone-shaped window the first time a picture arrives, within the screen.
        const scale = Math.min(avail.height * 0.85 / frame.height, avail.width * 0.85 / frame.width, 1)
        setSize(Math.max(minimumWidth, Math.round(frame.width * scale)),
                Math.max(minimumHeight, Math.round(frame.height * scale)))
        sized = true
        if (info.app)
            Mirror.saveGeometry(mirrorKey, x, y, width, height)
    }
    width: 380
    height: 820
    // Wayland gives no app a way to keep its window above others.
    readonly property bool canKeepOnTop: !Qt.platform.pluginName.startsWith("wayland")
    readonly property bool staysOnTop: (win.flags & Qt.WindowStaysOnTopHint) !== 0
    function toggleStayOnTop() {
        win.flags = staysOnTop ? (win.flags & ~Qt.WindowStaysOnTopHint) : (win.flags | Qt.WindowStaysOnTopHint)
    }

    onClosing: {
        if (info.app && sized && ownSize)
            Mirror.saveGeometry(mirrorKey, x, y, width, height)
        Mirror.stop(mirrorKey)
    }

    // ---- The phone's screen: the whole window ----
    Item {
        id: phoneScreen
        anchors.fill: parent

        VideoView {
            id: video
            anchors.fill: parent
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
                if (event.key === Qt.Key_F10 || event.key === Qt.Key_Menu) {
                    win.openTools()
                    event.accepted = true
                    return
                }
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

        // How to control it, until the phone allows it. The text sits above
        // its buttons, so it fits a window of any width.
        Rectangle {
            id: hintBanner
            objectName: "controlHint"
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: 10
            height: hintColumn.implicitHeight + 16
            radius: Theme.radiusMd
            visible: win.showing && !win.canControl && !win.info.app && !hintClose.closed
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

            // Clicks on the banner stay off the phone under it.
            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.AllButtons
            }

            Column {
                id: hintColumn
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.leftMargin: 10
                anchors.rightMargin: 6
                spacing: 4
                Txt {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    role: "bodySmall"
                    text: qsTr("To control it, turn on Control from your PC in G7 Link on the phone.")
                }
                Item {
                    width: parent.width
                    height: Math.max(notifyPhoneBtn.height, hintClose.height)
                    Button {
                        id: notifyPhoneBtn
                        objectName: "notifyPhone"
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(implicitWidth, parent.width - hintClose.width - 4)
                        variant: "tonal"
                        size: "sm"
                        text: qsTr("Notify phone")
                        onClicked: Mirror.requestControl(win.mirrorKey)
                    }
                    IconButton {
                        id: hintClose
                        objectName: "hideHint"
                        property bool closed: false
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        iconPath: Icons.close
                        label: qsTr("Hide")
                        onClicked: closed = true
                    }
                }
            }
        }

        // Before the picture: waiting for the phone, or why it ended.
        Rectangle {
            anchors.fill: parent
            visible: !win.showing
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

    // The phone's link, for the tools menu ("57 fps · 4 ms").
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

    // Everything the window can do, for the tools menu: the phone's keys
    // first (they need control from this PC, like the picture itself).
    // Shortcuts are left out of a narrow window, where the names need the room.
    function toolActions() {
        const list = []
        const roomy = win.width >= 360
        if (win.canControl) {
            list.push({ label: qsTr("Back (right-click)"), icon: Icons.back,
                        run: () => Mirror.press(win.mirrorKey, "back") })
            if (!win.info.app) {
                list.push({ label: qsTr("Home (middle-click)"), icon: Icons.home,
                            run: () => Mirror.press(win.mirrorKey, "home") })
                list.push({ label: qsTr("Recent apps"), icon: Icons.apps,
                            run: () => Mirror.press(win.mirrorKey, "recents") })
            }
        }
        list.push({ label: qsTr("Copy screenshot"), icon: Icons.copy, tint: Theme.tintBlue, keys: roomy ? "Ctrl Shift C" : "",
                    run: () => Mirror.screenshotClipboard(win.mirrorKey) })
        list.push({ label: qsTr("Save screenshot"), icon: Icons.screenshot, tint: Theme.tintBlue, keys: roomy ? "Ctrl S" : "",
                    run: () => Mirror.screenshotFile(win.mirrorKey) })
        list.push({ label: win.info.recording === true ? qsTr("Stop recording") : qsTr("Record video"),
                    icon: win.info.recording === true ? Icons.stopSquare : Icons.record, tint: Theme.tintRed, keys: roomy ? "Ctrl R" : "",
                    run: () => Mirror.toggleRecording(win.mirrorKey) })
        // Muted, no sound comes in: this brings it back.
        if (win.info.sound === true || (Mirror.muted && !win.info.app))
            list.push({ label: Mirror.muted ? qsTr("Play the phone's sound here") : qsTr("Play the sound on the phone"),
                        icon: Mirror.muted ? Icons.soundOff : Icons.speaker, tint: Theme.tintTeal,
                        run: () => Mirror.toggleSound() })
        if (win.canKeepOnTop)
            list.push({ label: qsTr("Keep on top"), icon: Icons.pin, tint: Theme.tintBlue, checked: win.staysOnTop,
                        run: () => win.toggleStayOnTop() })
        list.push({ label: qsTr("Keep phone awake"), icon: Icons.coffee, tint: Theme.tintAmber, checked: win.info.stayAwake === true,
                    run: () => Mirror.setStayAwake(win.mirrorKey, !(win.info.stayAwake === true)) })
        if (win.info.canScreenOff === true)
            list.push({ label: qsTr("Phone screen off"), icon: Icons.screenOff, tint: Theme.tintLavender,
                        checked: win.info.screenOff === true,
                        run: () => Mirror.setScreenOff(win.mirrorKey, !(win.info.screenOff === true)) })
        if (win.canControl)
            list.push({ label: qsTr("Lock phone"), icon: Icons.lock, tint: Theme.tintTeal,
                        run: () => Mirror.press(win.mirrorKey, "lock") })
        if (!win.canControl && !win.info.app)
            list.push({ label: qsTr("Ask the phone for control"), icon: Icons.gesture, tint: Theme.tintBlue,
                        run: () => Mirror.requestControl(win.mirrorKey) })
        list.push({ label: win.visibility === Window.FullScreen ? qsTr("Leave full screen") : qsTr("Full screen"),
                    icon: win.visibility === Window.FullScreen ? Icons.fullscreenExit : Icons.fullscreen,
                    keys: roomy ? "F11" : "", run: () => win.toggleFullScreen() })
        // How it's doing, for whoever wonders: not something to click.
        if (win.stats.length > 0)
            list.push({ label: win.stats, icon: Icons.mirror, enabled: false })
        return list
    }

    function openTools() {
        if (!win.showing)
            return
        toolsMenu.actions = win.toolActions()
        toolsMenu.openAt(Math.round((win.width - toolsMenu.panelWidth) / 2), toolsButton.y + toolsButton.height, false, false)
    }

    // ---- The tools: one button, there while the pointer moves over the window ----
    property bool pointerActive: false
    HoverHandler {
        id: windowHover
        // Where the pointer last was: Qt reports it again, unmoved, whenever
        // the picture under it changes, and that isn't the user reaching for it.
        property point seen: Qt.point(-1, -1)
        onHoveredChanged: {
            if (!hovered) {
                seen = Qt.point(-1, -1)
                win.pointerActive = false
            }
        }
        onPointChanged: {
            const at = point.position
            if (!hovered || (at.x === seen.x && at.y === seen.y))
                return
            const first = seen.x < 0
            seen = Qt.point(at.x, at.y)
            // Entering counts once the pointer moves on from where it came in.
            if (first)
                return
            win.pointerActive = true
            restTimer.restart()
        }
    }
    // A pointer that rests on the picture leaves it clear again.
    Timer {
        id: restTimer
        interval: 2500
        onTriggered: win.pointerActive = false
    }

    Rectangle {
        id: toolsButton
        objectName: "toolsButton"
        readonly property bool wanted: win.showing && (win.pointerActive || toolsHover.hovered || toolsMenu.opened)
        anchors.horizontalCenter: parent.horizontalCenter
        y: 8
        width: 44
        height: 24
        radius: height / 2
        color: Qt.rgba(0, 0, 0, toolsHover.hovered ? 0.78 : 0.56)
        border.width: 1
        border.color: Qt.rgba(1, 1, 1, 0.22)
        opacity: wanted ? 1 : 0
        visible: opacity > 0
        Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }

        Accessible.role: Accessible.Button
        Accessible.name: qsTr("More")
        Accessible.onPressAction: win.openTools()

        Icon {
            anchors.centerIn: parent
            width: 16; height: 16
            path: Icons.more
            color: "white"
        }
        HoverHandler {
            id: toolsHover
            cursorShape: Qt.PointingHandCursor
        }
        // Over the picture's own mouse area, so a click here isn't a tap on the phone.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
            onClicked: win.openTools()
        }
    }

    // Recording shows for as long as it runs, pointer or not.
    Rectangle {
        visible: win.info.recording === true
        x: 8
        y: 8
        width: recordingRow.implicitWidth + 14
        height: 24
        radius: height / 2
        color: Qt.rgba(0, 0, 0, 0.56)
        border.width: 1
        border.color: Qt.rgba(1, 1, 1, 0.22)
        Row {
            id: recordingRow
            anchors.centerIn: parent
            spacing: 6
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 8; height: 8; radius: 4
                color: Theme.tintRed
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: win.formatDuration(win.recordingElapsedSecs)
                role: "bodySmall"
                color: "white"
            }
        }
    }

    PopupMenu {
        id: toolsMenu
        objectName: "toolsMenu"
        panelWidth: 300
        wrapLabels: true
        onOpenedChanged: if (!opened) video.forceActiveFocus()
    }
}
