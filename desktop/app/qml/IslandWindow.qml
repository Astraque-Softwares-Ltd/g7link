// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import app.g7link

// A capsule at the top-center of the screen, hidden when idle, for live
// activity on the phone:
//   1. A call: caller, Decline / Answer while it rings, Mute / Hang up after
//   2. A transfer: file name, a thin progress bar and its percentage
//   3. Media playing on the phone: title, artist, Play / Pause, Previous / Next
//   4. New notifications, and transfers waiting for the phone
Window {
    id: islandWin

    // GNOME on Wayland lets no app keep its own window on top. The "PiP on top"
    // extension does it for windows titled like this, for those who have it.
    title: onWayland ? qsTr("G7 Island") + " - PiP" : qsTr("G7 Island")
    // Clicks pass through while it's waiting with nothing to show (see keptForMinutes).
    flags: Qt.Tool | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.WindowDoesNotAcceptFocus
        | (hasActivity ? 0 : Qt.WindowTransparentForInput)
    color: "transparent"

    readonly property int availW: Screen.desktopAvailableWidth > 0 ? Screen.desktopAvailableWidth : 1920

    // The work area (the screen minus the taskbar, wherever the taskbar is). Qt only knows
    // its size, so the app reports the position; refreshed when the work area changes.
    property var workArea: AppController.workArea()
    readonly property int workAreaKey: Screen.desktopAvailableWidth * 100000 + Screen.desktopAvailableHeight
    onWorkAreaKeyChanged: workArea = AppController.workArea()
    readonly property real dpr: Screen.devicePixelRatio > 0 ? Screen.devicePixelRatio : 1
    readonly property real areaX: workArea && workArea.width ? workArea.x / dpr : Screen.virtualX
    readonly property real areaY: workArea && workArea.height ? workArea.y / dpr : Screen.virtualY
    readonly property real areaW: workArea && workArea.width ? workArea.width / dpr : availW

    // ---- Track active media across all rows of MediaList (Phone + This PC) ----
    property string mediaDeviceId: ""
    property string mediaPlayerId: ""
    property string mediaApp: ""
    property string mediaTitle: ""
    property string mediaArtist: ""
    property string mediaArt: ""
    property bool mediaPlaying: false
    property bool mediaCanPlay: true
    property bool mediaCanPause: true
    property bool mediaCanNext: true
    property bool mediaCanPrev: true
    property real mediaDuration: 0
    property real mediaPosition: 0
    // When mediaPosition was current, as JavaScript time.
    property real mediaPositionAt: 0
    // Phones only report the position when it jumps, so it's counted forward here.
    property real mediaNow: Date.now()
    readonly property real mediaPositionNow: mediaPosition
        + (mediaPlaying ? Math.max(0, mediaNow - mediaPositionAt) : 0)

    Timer {
        interval: 1000
        repeat: true
        running: islandWin.mediaPlaying && islandWin.mediaDuration > 0 && islandWin.mode === "media"
        onRunningChanged: if (running) islandWin.mediaNow = Date.now()
        onTriggered: islandWin.mediaNow = Date.now()
    }

    // ---- Track active file transfer ----
    property string activeTransferId: ""
    property string activeTransferTitle: ""
    // The last one's title, kept for "Transfer complete": its row can go
    // before the count of active transfers drops.
    property string finishedTransferTitle: ""
    property real activeTransferProgress: 0
    property real activeTransferRate: 0
    property bool activeTransferIncoming: false
    property bool transferJustFinished: false
    property bool transferFailed: false
    readonly property color transferEndColor: transferFailed ? Theme.tintRed : Theme.tintGreen

    // ---- Transient notification toast (only when a new phone notification arrives) ----
    property bool notificationToastActive: false
    property int lastNotificationCount: NotificationList.count
    Timer {
        id: notificationToastTimer
        interval: 4200
        repeat: false
        onTriggered: islandWin.notificationToastActive = false
    }
    Connections {
        target: NotificationList
        function onCountChanged() {
            const cur = NotificationList.count
            if (cur > islandWin.lastNotificationCount && Preferences.g7IslandNotifications
                && !AppController.notificationsPaused) {
                islandWin.notificationToastActive = true
                notificationToastTimer.restart()
            } else if (cur === 0) {
                islandWin.notificationToastActive = false
            }
            islandWin.lastNotificationCount = cur
        }
    }

    // QSG_RENDER_LOOP=basic advances QML's animation clock only while a window
    // renders; with the main window closed (App.qml) the Island is alone, so
    // its clock freezes. A wall-clock timer repaints each frame to re-advance
    // it, but only while something is really moving: the looping equalizer or
    // scrolling title, or a transition. A still pill is left alone.
    readonly property bool animating:
        (mode === "media" && mediaPlaying && !expanded && !Theme.reduceMotion)
        || compactTitleClip.shouldMarquee
        || transitionSettle.running

    // Holds `animating` true through the drop-in and expand/collapse Behaviors,
    // which would otherwise snap while the clock is frozen.
    Timer {
        id: transitionSettle
        interval: 320
        repeat: false
    }
    onExpandedChanged: if (!Theme.reduceMotion) transitionSettle.restart()

    Timer {
        running: islandWin.visible && islandWin.animating
        interval: 16
        repeat: true
        onTriggered: islandWin.update()
    }

    Timer {
        id: transferDoneTimer
        interval: 3200
        repeat: false
        onTriggered: {
            islandWin.transferJustFinished = false
            islandWin.transferFailed = false
            islandWin.finishedTransferTitle = ""
        }
    }

    // Rows change before `active` does, so by the time this runs every transfer
    // that just ended has been through noteTransferEnded(). Nothing is shown when
    // they were all cancelled.
    property int lastActiveTransfers: TransferList.active
    onLastActiveTransfersChanged: {
        if (lastActiveTransfers > 0) {
            // A new transfer replaces a banner still showing, and starts afresh.
            if (transferJustFinished) {
                transferDoneTimer.stop()
                transferJustFinished = false
                transferFailed = false
                finishedTransferTitle = ""
            }
            return
        }
        if (finishedTransferTitle.length > 0) {
            transferJustFinished = true
            transferDoneTimer.restart()
        }
    }

    function noteTransferEnded(status, title) {
        if (status === "failed") {
            transferFailed = true
            finishedTransferTitle = title
        } else if (status === "done" && !transferFailed) {
            finishedTransferTitle = title
        }
    }

    function recomputeMedia() {
        let best = null
        const count = mediaRepeater.count
        for (let i = 0; i < count; i++) {
            const item = mediaRepeater.itemAt(i)
            if (!item) continue
            // Only surface phone media in G7 Island (never PC-local media, which is already on the PC)
            if (item.deviceId === "local") continue
            if (item.playing) {
                if (!best || !best.playing) {
                    best = item
                }
            }
        }
        if (best) {
            mediaDeviceId = best.deviceId
            mediaPlayerId = best.playerId
            mediaApp = best.app
            mediaTitle = best.title
            mediaArtist = best.artist
            mediaArt = best.art
            mediaPlaying = best.playing
            mediaCanPlay = best.canPlay
            mediaCanPause = best.canPause
            mediaCanNext = best.canNext
            mediaCanPrev = best.canPrevious
            mediaDuration = best.duration
            mediaPosition = best.position
            mediaPositionAt = best.positionAt
            mediaNow = Date.now()
        } else {
            mediaDeviceId = ""
            mediaPlayerId = ""
            mediaApp = ""
            mediaTitle = ""
            mediaArtist = ""
            mediaArt = ""
            mediaPlaying = false
        }
    }

    function recomputeTransfer() {
        let found = null
        const count = transferRepeater.count
        for (let i = 0; i < count; i++) {
            const item = transferRepeater.itemAt(i)
            if (!item) continue
            if (item.status === "running" || item.status === "waiting" || item.status === "requested" || item.status === "paused") {
                found = item
                break
            }
        }
        if (found) {
            activeTransferId = found.transferId
            activeTransferTitle = found.title
            activeTransferProgress = Math.max(0, Math.min(1, found.progress || 0))
            activeTransferRate = found.rate || 0
            activeTransferIncoming = found.incoming
        } else if (!transferJustFinished) {
            activeTransferId = ""
            activeTransferTitle = ""
            activeTransferProgress = 0
            activeTransferRate = 0
        }
    }

    function formatRate(bytesPerSec) {
        const b = Number(bytesPerSec) || 0
        if (b <= 0) return ""
        if (b < 1024 * 1024)
            return Math.round(b / 1024) + " KB/s"
        return Qt.locale().toString(b / (1024 * 1024), "f", 1) + " MB/s"
    }

    readonly property bool hasMediaCandidate: mediaPlaying && mediaDeviceId !== "local" && mediaTitle.length > 0
    readonly property string mode: {
        if (Preferences.g7IslandCalls && PhoneCall.active) return "call"
        if (Preferences.g7IslandTransfers && (TransferList.active > 0 || transferJustFinished)) return "transfer"
        if (Preferences.g7IslandMedia && hasMediaCandidate) return "media"
        if (Preferences.g7IslandNotifications && notificationToastActive && NotificationList.count > 0) return "notification"
        if (Preferences.g7IslandTransfers && TransferList.waitingCount > 0) return "waiting"
        return "idle"
    }

    readonly property bool hasActivity: mode !== "idle"
    readonly property bool onWayland: Qt.platform.pluginName.startsWith("wayland")
    // Wayland lets the Island be dragged, but a window that hides comes back
    // wherever the compositor puts it. So after activity it stays, empty and
    // letting clicks through, for a while: a song paused and played again keeps
    // its spot. Then it hides, out of the window switcher.
    property bool keptForMinutes: false
    onHasActivityChanged: {
        if (hasActivity) {
            keepTimer.stop()
            // Coming back while kept: the window is already up, so drop in here.
            if (keptForMinutes && visible && !Theme.reduceMotion)
                dropInAnim.restart()
            keptForMinutes = onWayland
        } else if (keptForMinutes) {
            keepTimer.restart()
        }
    }
    Timer {
        id: keepTimer
        interval: 10 * 60 * 1000
        onTriggered: islandWin.keptForMinutes = false
    }
    visible: Preferences.g7Island && (hasActivity || keptForMinutes)

    onVisibleChanged: {
        if (!visible) {
            keepTimer.stop()
            keptForMinutes = false
        }
        if (visible) {
            Qt.callLater(() => AppController.styleShelfWindow())
            if (!Theme.reduceMotion) {
                dropInAnim.restart()
                transitionSettle.restart()
            }
        }
    }

    Component.onCompleted: {
        Qt.callLater(() => {
            recomputeMedia()
            recomputeTransfer()
            AppController.styleShelfWindow()
        })
    }

    Connections {
        target: Preferences
        function onG7IslandChanged() {
            if (Preferences.g7Island) {
                Qt.callLater(() => AppController.styleShelfWindow())
            }
        }
    }

    readonly property bool expanded: pillHover.hovered || (Preferences.g7IslandCalls && PhoneCall.active && PhoneCall.ringing)
    // Each activity has its own width (call 380, transfer 320, media and the
    // rest 300); hovering widens it for the extra controls.
    readonly property real pillWidth: (mode === "call" ? 380 : (mode === "transfer" ? 320 : 300)) + (expanded && mode !== "call" ? 60 : 0)
    readonly property real pillHeight: 40
    // The activity's glyph and its tint.
    readonly property string glyph: mode === "call" ? Icons.call
        : mode === "transfer" ? (transferJustFinished ? (transferFailed ? Icons.close : Icons.check)
                                                      : (activeTransferIncoming ? Icons.download : Icons.send))
        : mode === "media" ? Icons.music
        : mode === "notification" ? Icons.bell
        : mode === "waiting" ? Icons.send : Icons.phone
    readonly property color glyphTint: mode === "call" ? Theme.tintTeal
        : mode === "transfer" ? (transferJustFinished ? transferEndColor : Theme.tintAmber)
        : mode === "media" ? Theme.tintPink
        : mode === "notification" ? Theme.tintBlue : Theme.accent
    // What follows the label in the capsule: "calling", the artist, or the
    // secondary label when there's room for it.
    readonly property string detailLabel: {
        if (mode === "call")
            return PhoneCall.ringing ? qsTr("calling") : secondaryLabel
        if (mode === "transfer")
            return transferJustFinished && TransferList.active === 0 ? finishedTransferTitle
                : (expanded ? secondaryLabel : "")
        if (mode === "media")
            return mediaArtist.length > 0 ? mediaArtist : mediaApp
        return expanded ? secondaryLabel : ""
    }
    width: 400
    height: 104
    x: areaX + Math.round((areaW - width) / 2)
    y: areaY + 8

    readonly property string primaryLabel: {
        if (mode === "call")
            return PhoneCall.caller.length > 0 ? PhoneCall.caller : qsTr("Phone call")
        if (mode === "transfer") {
            if (transferJustFinished && TransferList.active === 0)
                return transferFailed ? qsTr("Transfer failed") : qsTr("Transfer complete")
            return activeTransferTitle.length > 0
                ? activeTransferTitle
                : qsTr("Transferring %1 file(s)…").arg(TransferList.active)
        }
        if (mode === "media")
            return mediaTitle.length > 0 ? mediaTitle : (mediaApp.length > 0 ? mediaApp : qsTr("Now playing"))
        if (mode === "notification")
            return qsTr("%1 phone notification(s)").arg(NotificationList.count)
        if (mode === "waiting")
            return qsTr("%1 waiting").arg(TransferList.waitingCount)
        return DeviceList.count > 0
            ? qsTr("G7 Island · %1").arg(DeviceList.deviceNameAt(0))
            : qsTr("G7 Island · Ready")
    }

    readonly property string secondaryLabel: {
        if (mode === "call") {
            if (PhoneCall.ringing) return qsTr("Incoming call")
            if (PhoneCall.held) return qsTr("On hold")
            return PhoneCall.number.length > 0 ? PhoneCall.number : qsTr("Call in progress")
        }
        if (mode === "transfer") {
            if (transferJustFinished && TransferList.active === 0)
                return finishedTransferTitle
            const pct = Math.round(activeTransferProgress * 100) + "%"
            const spd = formatRate(activeTransferRate)
            const base = spd.length > 0 ? (pct + " · " + spd) : pct
            return TransferList.waitingCount > 0
                ? (base + " · " + qsTr("%1 waiting").arg(TransferList.waitingCount))
                : base
        }
        if (mode === "media") {
            const src = mediaDeviceId === "local" ? qsTr("This PC") : qsTr("Phone")
            const base = mediaArtist.length > 0
                ? (mediaArtist + " · " + src)
                : (mediaApp.length > 0 ? (mediaApp + " · " + src) : src)
            return TransferList.waitingCount > 0
                ? (base + " · " + qsTr("%1 waiting").arg(TransferList.waitingCount))
                : base
        }
        if (mode === "notification")
            return qsTr("Click to view in G7 Link")
        if (mode === "waiting") {
            return DeviceList.count > 0
                ? qsTr("Will send when %1 is back").arg(DeviceList.deviceNameAt(0))
                : qsTr("Will send when connected")
        }
        return qsTr("Live calls, transfers & media")
    }

    Item {
        id: root
        anchors.fill: parent
        visible: islandWin.hasActivity

        // Hidden repeaters inside Item scope so QtQuick instantiates delegates cleanly
        Repeater {
            id: mediaRepeater
            model: Preferences.g7Island ? MediaList : null
            onItemAdded: islandWin.recomputeMedia()
            onItemRemoved: islandWin.recomputeMedia()
            delegate: Item {
                required property int index
                required property string deviceId
                required property string playerId
                required property string app
                required property string title
                required property string artist
                required property string art
                required property bool playing
                required property bool canPlay
                required property bool canPause
                required property bool canNext
                required property bool canPrevious
                required property real duration
                required property real position
                required property real positionAt
                visible: false
                onPlayingChanged: islandWin.recomputeMedia()
                onTitleChanged: islandWin.recomputeMedia()
                onArtistChanged: islandWin.recomputeMedia()
                onArtChanged: islandWin.recomputeMedia()
                onDeviceIdChanged: islandWin.recomputeMedia()
                onPositionChanged: islandWin.recomputeMedia()
                onDurationChanged: islandWin.recomputeMedia()
                onCanPlayChanged: islandWin.recomputeMedia()
                onCanPauseChanged: islandWin.recomputeMedia()
                onCanNextChanged: islandWin.recomputeMedia()
                onCanPreviousChanged: islandWin.recomputeMedia()
            }
        }

        Repeater {
            id: transferRepeater
            model: Preferences.g7Island ? TransferList : null
            onItemAdded: islandWin.recomputeTransfer()
            onItemRemoved: islandWin.recomputeTransfer()
            delegate: Item {
                required property int index
                required property string transferId
                required property string title
                required property real progress
                required property real rate
                required property bool incoming
                required property string status
                visible: false
                // Creating a row sets its status too, and an old row says nothing new.
                property bool created: false
                Component.onCompleted: created = true
                onStatusChanged: {
                    if (created)
                        islandWin.noteTransferEnded(status, title)
                    islandWin.recomputeTransfer()
                }
                onProgressChanged: islandWin.recomputeTransfer()
                onRateChanged: islandWin.recomputeTransfer()
                onTitleChanged: islandWin.recomputeTransfer()
            }
        }

        transform: Translate {
            id: dropSlide
            y: 0
        }

        NumberAnimation {
            id: dropInAnim
            target: dropSlide
            property: "y"
            from: -18
            to: 0
            duration: 240
            easing.type: Easing.OutBack
        }

        // One capsule on the ground color with a 1 px outline: a glyph in its
        // tint, the label, muted detail, and the activity's controls on the
        // right. Hovering widens it and shows the rest of the controls.
        Rectangle {
            id: pillBg
            anchors.top: parent.top
            anchors.topMargin: 2
            anchors.horizontalCenter: parent.horizontalCenter
            width: islandWin.pillWidth
            height: islandWin.pillHeight
            radius: height / 2
            color: Theme.surface
            border.width: 1
            border.color: Theme.line
            clip: true

            Behavior on width {
                enabled: !Theme.reduceMotion
                NumberAnimation { duration: 180; easing.type: Easing.OutCubic }
            }

            HoverHandler {
                id: pillHover
                cursorShape: Qt.PointingHandCursor
            }

            // Wayland ignores where we ask to put the window, so let it be dragged.
            DragHandler {
                target: null
                enabled: Qt.platform.pluginName === "wayland"
                onActiveChanged: if (active) islandWin.startSystemMove()
            }

            TapHandler {
                onTapped: (eventPoint) => {
                    const p = controls.mapFromItem(pillBg, eventPoint.position)
                    if (controls.visible && p.x >= 0 && p.y >= 0 && p.x <= controls.width && p.y <= controls.height)
                        return
                    AppController.activateRequested()
                }
            }

            // The activity's glyph, or the song's artwork.
            Item {
                id: lead
                x: 14
                anchors.verticalCenter: parent.verticalCenter
                width: 18; height: 18
                Icon {
                    anchors.fill: parent
                    visible: !(islandWin.mode === "media" && islandWin.mediaArt.length > 0 && artImage.status === Image.Ready)
                    path: islandWin.glyph
                    color: islandWin.glyphTint
                }
                RoundedImage {
                    id: artImage
                    anchors.fill: parent
                    visible: islandWin.mode === "media" && islandWin.mediaArt.length > 0
                    radius: 5
                    source: islandWin.mode === "media" ? islandWin.mediaArt : ""
                    sourceSize: Qt.size(48, 48)
                }
            }

            Txt {
                id: capsuleTitle
                anchors.left: lead.right
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, Math.max(40, controls.x - x - 10 - (capsuleDetail.text.length > 0 ? Math.min(capsuleDetail.implicitWidth, 120) + 10 : 0)))
                text: islandWin.primaryLabel
                role: "bodySmall"
                weight: 500
            }
            Txt {
                id: capsuleDetail
                anchors.left: capsuleTitle.right
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                width: Math.max(0, controls.x - x - 10)
                text: islandWin.detailLabel
                role: "bodySmall"
                muted: true
            }

            Row {
                id: controls
                anchors.right: parent.right
                anchors.rightMargin: islandWin.mode === "call" ? 6 : 14
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6

                // ---- A call: Decline and Answer while it rings, then Mute and Hang up ----
                IslandPill {
                    visible: islandWin.mode === "call" && PhoneCall.ringing && PhoneCall.canEnd
                    enabled: PhoneCall.pending === ""
                    text: PhoneCall.pending === "end" ? qsTr("Ending…") : qsTr("Decline")
                    fill: Theme.tintRed
                    onActivated: PhoneCall.end()
                }
                IslandPill {
                    visible: islandWin.mode === "call" && PhoneCall.ringing && PhoneCall.canEnd
                    enabled: PhoneCall.pending === ""
                    text: PhoneCall.pending === "answer" ? qsTr("Answering…") : qsTr("Answer")
                    fill: Theme.tintTeal
                    onActivated: PhoneCall.answer()
                }
                IslandIcon {
                    visible: islandWin.mode === "call" && !PhoneCall.ringing && PhoneCall.controls
                    iconPath: PhoneCall.muted ? Icons.micOff : Icons.mic
                    tint: PhoneCall.muted ? Theme.tintRed : Theme.surfaceContent
                    label: PhoneCall.muted ? qsTr("Unmute") : qsTr("Mute")
                    onActivated: PhoneCall.setMute(!PhoneCall.muted)
                }
                IslandPill {
                    visible: islandWin.mode === "call" && !PhoneCall.ringing && PhoneCall.canEnd
                    enabled: PhoneCall.pending === ""
                    text: PhoneCall.pending === "end" ? qsTr("Ending…") : qsTr("Hang up")
                    fill: Theme.tintRed
                    onActivated: PhoneCall.end()
                }

                // ---- A transfer: a thin bar and its percentage; Cancel on hover ----
                Rectangle {
                    visible: islandWin.mode === "transfer"
                    anchors.verticalCenter: parent.verticalCenter
                    width: 60; height: 4; radius: 2
                    color: Theme.selection
                    Rectangle {
                        height: parent.height
                        radius: parent.radius
                        color: islandWin.transferJustFinished ? islandWin.transferEndColor : Theme.tintAmber
                        width: parent.width * (islandWin.transferJustFinished ? 1.0 : Math.max(0.04, islandWin.activeTransferProgress))
                        Behavior on width { enabled: !Theme.reduceMotion; NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                    }
                }
                Txt {
                    visible: islandWin.mode === "transfer" && !islandWin.transferJustFinished
                    anchors.verticalCenter: parent.verticalCenter
                    text: Math.round(islandWin.activeTransferProgress * 100) + "%"
                    role: "bodySmall"
                    muted: true
                }
                IslandIcon {
                    visible: islandWin.expanded && islandWin.mode === "transfer" && !islandWin.transferJustFinished
                             && islandWin.activeTransferId.length > 0
                    iconPath: Icons.close
                    label: qsTr("Cancel")
                    onActivated: TransferList.cancel(islandWin.activeTransferId)
                }

                // ---- Media: Play or pause always; Previous and Next on hover ----
                IslandIcon {
                    visible: islandWin.expanded && islandWin.mode === "media" && islandWin.mediaCanPrev
                    iconPath: Icons.skipPrevious
                    label: qsTr("Previous")
                    onActivated: MediaList.command(islandWin.mediaDeviceId, islandWin.mediaPlayerId, "previous", 0)
                }
                IslandIcon {
                    visible: islandWin.mode === "media"
                    iconPath: islandWin.mediaPlaying ? Icons.pause : Icons.play
                    label: islandWin.mediaPlaying ? qsTr("Pause") : qsTr("Play")
                    onActivated: MediaList.command(islandWin.mediaDeviceId, islandWin.mediaPlayerId,
                                                   islandWin.mediaPlaying ? "pause" : "play", 0)
                }
                IslandIcon {
                    visible: islandWin.mode === "media" && islandWin.mediaCanNext
                    iconPath: Icons.skipNext
                    label: qsTr("Next")
                    onActivated: MediaList.command(islandWin.mediaDeviceId, islandWin.mediaPlayerId, "next", 0)
                }
            }

            // Media: how far into the song, as a hairline along the bottom.
            Rectangle {
                visible: islandWin.mode === "media" && islandWin.mediaDuration > 0
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 1
                x: 18
                width: (parent.width - 36) * Math.max(0, Math.min(1, islandWin.mediaPositionNow / Math.max(1, islandWin.mediaDuration)))
                height: 1
                color: Theme.tintPink
            }
        }
    }

    // A filled pill: the call's Decline, Answer and Hang up.
    component IslandPill: Rectangle {
        id: ip
        property string text
        property color fill: Theme.accent
        signal activated
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        width: ipText.implicitWidth + 24
        height: 28
        radius: 14
        opacity: enabled ? 1 : 0.6
        color: ipHover.hovered ? Qt.lighter(fill, 1.08) : fill
        Accessible.role: Accessible.Button
        Accessible.name: text
        Txt {
            id: ipText
            anchors.centerIn: parent
            text: ip.text
            role: "bodySmall"
            weight: 500
            color: Theme.surface
        }
        HoverHandler { id: ipHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { onTapped: ip.activated() }
    }

    // A bare glyph button.
    component IslandIcon: Rectangle {
        id: ii
        property string iconPath
        property string label
        property color tint: Theme.surfaceContent
        signal activated
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        width: 26; height: 26
        radius: 13
        color: iiHover.hovered ? Theme.selection : "transparent"
        Accessible.role: Accessible.Button
        Accessible.name: label
        Icon {
            anchors.centerIn: parent
            width: 15; height: 15
            path: ii.iconPath
            color: ii.tint
        }
        HoverHandler { id: iiHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { onTapped: ii.activated() }
    }
}
