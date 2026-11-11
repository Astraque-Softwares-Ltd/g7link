// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import QtQuick.Dialogs
import app.g7link

// Home: the phone's notifications and what you can do with it on the left;
// on the right the phone itself: link, battery, controls, what's playing,
// and a call while one rings. Every action is also in Actions (Ctrl B).
Item {
    id: page
    property bool active: true
    property int current: AppController.currentDevice
    Connections {
        target: AppController
        function onCurrentDeviceChanged() {
            if (page.current !== AppController.currentDevice)
                page.current = AppController.currentDevice
        }
    }
    // The device shown, for dropped files.
    property string currentDeviceId
    property string currentDeviceName
    property bool currentCanReceive: false
    signal pairRequested

    // ---- The footer and Actions (Ctrl B) ----
    // The phone on screen fills these in (DeviceHome).
    property var phonePrimary: null
    property var phoneSecondary: null
    property var phoneActions: []
    readonly property string footerLabel: ""
    readonly property var primaryAction: DeviceList.count > 0
        ? phonePrimary
        : ({ label: qsTr("Pair a phone"), keys: "", run: () => page.pairRequested() })
    readonly property var secondaryAction: DeviceList.count > 0 ? phoneSecondary : null
    readonly property var screenActions: (DeviceList.count > 0 ? phoneActions : []).concat([
        { label: qsTr("Pair a phone"), icon: Icons.plus, tint: Theme.accent, run: () => page.pairRequested() },
        { label: qsTr("Features guide"), icon: Icons.book, tint: Theme.tintBlue,
          run: () => { AppController.currentPage = "guide" } },
        { label: qsTr("Setup walkthrough"), icon: Icons.compass, tint: Theme.tintTeal,
          run: () => Preferences.restartWalkthrough() },
        { label: qsTr("Timeline"), icon: Icons.history, tint: Theme.tintLavender,
          run: () => { AppController.currentPage = "timeline" } },
        { label: qsTr("Clear finished transfers"), icon: Icons.close, tint: Theme.surfaceContentVariant,
          enabled: TransferList.count > TransferList.active, run: () => TransferList.clearFinished() },
        { label: qsTr("Refresh LocalSend"), icon: Icons.refresh, tint: Theme.surfaceContentVariant,
          enabled: AppController.localsendEnabled, run: () => AppController.refreshLocalsend() }
    ])

    // The notification opened to show its actions (by key).
    property string selectedNotification: ""

    readonly property var localsendPeers: {
        if (!AppController.localsendEnabled || !AppController.localsendPeersJson || AppController.localsendPeersJson.length === 0)
            return []
        try {
            return JSON.parse(AppController.localsendPeersJson)
        } catch (e) {
            return []
        }
    }

    // A feature can be used now: available, partly, or worth queuing for
    // later (files and clips wait for an offline phone), or a webcam whose
    // add-on is missing (it walks through setting it up).
    function featureUsable(f, online, queueable, addonOk) {
        const s = f || {}
        return s.state === "available" || s.state === "partial"
            || (addonOk && s.state === "locked" && s.action === "enableAddon")
            || (queueable && (!online || s.state === "unknown") && s.state !== "locked" && s.state !== "unsupported")
    }
    // "Ctrl+Alt+C" as a keycap shows it: "Ctrl Alt C".
    function keycap(hotkey) { return (hotkey || "").replace(/\+/g, " ") }

    // The send queue as a model that's updated in place, so its rows keep
    // their delegates as progress comes in. Items whose transfer already has
    // a row under Transfers are left out: one row per file.
    ListModel { id: sendQueueModel }
    property int sendQueueWaiting: 0

    function syncSendQueue() {
        let items = []
        try {
            items = TransferList.sendQueue && TransferList.sendQueue.length > 0
                ? JSON.parse(TransferList.sendQueue) : []
        } catch (e) {
            items = []
        }
        const rows = items
            .filter(item => !(item.status === "sending" && item.live))
            .map(item => ({
                queueId: String(item.id || ""),
                kind: String(item.kind || ""),
                icon: String(item.icon || ""),
                title: String(item.title || ""),
                status: String(item.status || "waiting"),
                reason: String(item.reason || ""),
                deviceName: String(item.deviceName || ""),
                totalBytes: Number(item.totalBytes || 0),
                progress: Number(item.progress || 0)
            }))
        const ids = rows.map(row => row.queueId)
        for (let r = sendQueueModel.count - 1; r >= 0; r--) {
            if (ids.indexOf(sendQueueModel.get(r).queueId) < 0)
                sendQueueModel.remove(r)
        }
        for (let i = 0; i < rows.length; i++) {
            let at = -1
            for (let r = i; r < sendQueueModel.count; r++) {
                if (sendQueueModel.get(r).queueId === rows[i].queueId) { at = r; break }
            }
            if (at < 0) {
                sendQueueModel.insert(i, rows[i])
            } else {
                if (at !== i) sendQueueModel.move(at, i, 1)
                sendQueueModel.set(i, rows[i])
            }
        }
        sendQueueWaiting = rows.filter(row => row.status === "waiting" || row.status === "sending").length
    }

    Connections {
        target: TransferList
        function onSendQueueChanged() { page.syncSendQueue() }
    }
    Component.onCompleted: syncSendQueue()

    function formatQueueSize(bytes) {
        if (!bytes || bytes <= 0) return ""
        if (bytes < 1024) return qsTr("%1 B").arg(Math.round(bytes))
        const units = [qsTr("%1 KB"), qsTr("%1 MB"), qsTr("%1 GB"), qsTr("%1 TB")]
        let value = bytes / 1024
        let unit = 0
        while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++ }
        return units[unit].arg(value < 10 ? Qt.locale().toString(value, "f", 1) : Math.round(value))
    }

    FileDialog {
        id: localsendFilePicker
        property string targetDeviceId: ""
        title: qsTr("Send files via LocalSend")
        fileMode: FileDialog.OpenFiles
        onAccepted: {
            if (targetDeviceId.length > 0 && selectedFiles.length > 0)
                TransferList.send(targetDeviceId, selectedFiles.map(url => url.toString()))
        }
    }

    visible: active
    readonly property bool resizing: Window.window && Window.window.resizing ? true : false

    onCurrentChanged: {
        if (current >= DeviceList.count) current = 0
        if (AppController.currentDevice !== current)
            AppController.currentDevice = current
    }
    onCurrentDeviceIdChanged: AppController.watchHomeSummary(visible ? currentDeviceId : "")
    onVisibleChanged: AppController.watchHomeSummary(visible ? currentDeviceId : "")
    Component.onDestruction: AppController.watchHomeSummary("")
    Connections {
        target: DeviceList
        function onCountChanged() {
            if (page.current >= DeviceList.count)
                page.current = Math.max(0, DeviceList.count - 1)
        }
    }

    function relativeTime(unixSeconds) {
        if (!unixSeconds)
            return qsTr("never")
        const minutes = Math.round((Date.now() / 1000 - unixSeconds) / 60)
        if (minutes < 1) return qsTr("just now")
        if (minutes < 60) return qsTr("%n min ago", "", minutes)
        const hours = Math.round(minutes / 60)
        if (hours < 24) return qsTr("%n h ago", "", hours)
        return new Date(unixSeconds * 1000).toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }

    function formatChargeEta(minutes) {
        if (!minutes || minutes <= 0)
            return ""
        if (minutes < 60)
            return qsTr("~%1 min to full").arg(minutes)
        const h = Math.floor(minutes / 60)
        const m = minutes % 60
        return m > 0 ? qsTr("~%1h %2m to full").arg(h).arg(m) : qsTr("~%1h to full").arg(h)
    }

    // ---- No phone yet ----
    Flickable {
        anchors.fill: parent
        visible: DeviceList.count === 0
        contentHeight: noPhone.height + 32
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        Column {
            id: noPhone
            x: 8
            y: 16
            width: Math.min(parent.width - 16, 720)
            spacing: 6
            Item {
                width: parent.width
                height: 40
                Txt {
                    x: Theme.rowPadding
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("No phone paired")
                    weight: 500
                }
                Row {
                    anchors.right: parent.right
                    anchors.rightMargin: Theme.rowPadding
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 6
                    TextAction {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Features guide")
                        onClicked: AppController.currentPage = "guide"
                    }
                    TextAction {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Setup walkthrough")
                        onClicked: Preferences.restartWalkthrough()
                    }
                    Button {
                        size: "sm"
                        iconPath: Icons.plus
                        text: qsTr("Pair a phone")
                        onClicked: page.pairRequested()
                    }
                }
            }
            SectionLabel {
                width: parent.width
                visible: TransferList.count > 0
                text: qsTr("Transfers")
                actionText: TransferList.count > TransferList.active ? qsTr("Clear finished") : ""
                onActionClicked: TransferList.clearFinished()
            }
            Repeater {
                model: DeviceList.count === 0 ? TransferList : null
                delegate: TransferItem { width: noPhone.width }
            }
            LocalSendSection {
                width: parent.width
                visible: AppController.localsendEnabled && page.localsendPeers.length > 0
            }
        }
    }

    // ---- The phone on screen ----
    Repeater {
        model: DeviceList
        delegate: DeviceHome {
            required property int index
            visible: index === page.current
            width: page.width
            height: page.height
        }
    }

    // Files dragged onto the page are sent to the device on screen.
    // Holding Shift while dropping a single document hands it off so it opens
    // with the default app upon arrival.
    DropArea {
        id: drop
        anchors.fill: parent
        enabled: page.currentCanReceive
        onEntered: (drag) => drag.accepted = drag.hasUrls
        onDropped: (drag) => {
            if (drag.hasUrls) {
                const urls = drag.urls.map(url => url.toString())
                if ((drag.modifiers & Qt.ShiftModifier) && urls.length === 1)
                    TransferList.sendHandoff(page.currentDeviceId, urls)
                else
                    TransferList.send(page.currentDeviceId, urls)
                drag.accept(Qt.CopyAction)
            }
        }
    }
    Rectangle {
        anchors.fill: parent
        anchors.margins: 8
        radius: Theme.radiusXl
        visible: opacity > 0
        opacity: drop.containsDrag ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
        color: Qt.rgba(Theme.surface.r, Theme.surface.g, Theme.surface.b, 0.94)
        border.width: 1
        border.color: Theme.accent
        Column {
            anchors.centerIn: parent
            spacing: 8
            Icon {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 28; height: 28
                path: Icons.send
                color: Theme.tintAmber
            }
            Txt {
                anchors.horizontalCenter: parent.horizontalCenter
                weight: 500
                text: qsTr("Drop to send to %1").arg(page.currentDeviceName)
            }
            Txt {
                anchors.horizontalCenter: parent.horizontalCenter
                role: "bodySmall"
                muted: true
                text: qsTr("Shift: open a document on arrival")
            }
        }
    }

    component DeviceHome: Item {
        id: home
        required property string deviceId
        required property string name
        required property string rawName
        required property bool hasNickname
        required property string kind
        required property string osVersion
        required property string model
        required property bool online
        required property string path
        required property int rttMs
        required property int battery
        required property bool charging
        required property string batteryPlugged
        required property int batteryFullIn
        required property int cellBars
        required property string cellType
        required property int wifiBars
        required property string power
        required property real lastSeen
        required property real pairedAt
        required property string accent
        required property string screen
        required property bool reconnect

        // Dropped files go to the device on screen (and Messages shows it).
        // Kept while the page is hidden.
        Binding { target: page; property: "currentDeviceId"; value: home.deviceId; when: home.visible; restoreMode: Binding.RestoreNone }
        Binding { target: page; property: "currentDeviceName"; value: home.name; when: home.visible; restoreMode: Binding.RestoreNone }
        Binding {
            target: page
            property: "currentCanReceive"
            value: home.feature("files.send").state === "available"
                   || (!home.online
                       && home.feature("files.send").state !== "locked"
                       && home.feature("files.send").state !== "unsupported")
            when: home.visible
        }
        // The footer and Actions list follow the phone on screen.
        Binding { target: page; property: "phonePrimary"; value: home.primary; when: home.visible; restoreMode: Binding.RestoreNone }
        Binding { target: page; property: "phoneSecondary"; value: home.secondary; when: home.visible; restoreMode: Binding.RestoreNone }
        Binding { target: page; property: "phoneActions"; value: home.actionList; when: home.visible; restoreMode: Binding.RestoreNone }

        // Re-read capabilities whenever any matrix changes.
        function feature(id) {
            return AppController.capsRevision >= 0 ? AppController.featureState(deviceId, id) : ({})
        }
        function usable(id, queueable, addonOk) {
            return page.featureUsable(feature(id), home.online, queueable === true, addonOk === true)
        }

        property bool ringing: false
        // The phone stops ringing by itself after a minute (Ringer.MAX_RING_MS).
        Timer { id: ringTimeout; interval: 60000; onTriggered: home.ringing = false }

        readonly property bool callHere: PhoneCall.active && PhoneCall.device === home.deviceId
        readonly property bool ringingHere: callHere && PhoneCall.ringing
        readonly property bool mirroring: {
            try {
                return JSON.parse(Mirror.windows).some(w => w.device === home.deviceId && w.session === 0
                                                           && w.phase !== "ended")
            } catch (e) {
                return false
            }
        }
        readonly property bool webcamHere: (Webcam.phase === "streaming" || Webcam.phase === "asking")
                                           && Webcam.activeDevice === home.deviceId
        readonly property bool typingHere: Mirror.keyboardDevice === home.deviceId
        readonly property string phoneSound: MediaList.revision >= 0 ? MediaList.phoneSoundState(home.deviceId) : "off"

        function mirror() { Mirror.start(home.deviceId) }
        function findPhone() {
            home.ringing = !home.ringing
            if (home.ringing) ringTimeout.restart()
            AppController.ring(home.deviceId, home.ringing)
        }
        function toggleWebcam() {
            if (home.webcamHere)
                Webcam.stop()
            else
                Webcam.start(home.deviceId)
        }
        function toggleTyping() {
            if (home.typingHere) {
                Mirror.stopKeyboard()
            } else {
                Mirror.toggleKeyboard(home.deviceId)
                remoteKeyCapture.forceActiveFocus()
            }
        }
        function openClipboardHistory() {
            clipboardSearch.text = ""
            clipboardSheet.open()
        }
        function openNotificationHistory() {
            historySearch.text = ""
            historySheet.open()
        }

        readonly property var primary: home.ringingHere
            ? ({ label: qsTr("Answer"), keys: "Ctrl Enter", run: () => PhoneCall.answer() })
            : ({ label: qsTr("Mirror screen"), keys: "Ctrl M", run: () => home.mirror() })
        readonly property var secondary: home.ringingHere
            ? ({ label: qsTr("Decline"), keys: "Esc", run: () => PhoneCall.end() })
            : null
        readonly property var actionList: {
            const list = []
            if (home.callHere && PhoneCall.ringing) {
                list.push({ label: qsTr("Answer"), icon: Icons.call, tint: Theme.tintTeal, keys: "Ctrl Enter",
                            run: () => PhoneCall.answer() })
                list.push({ label: qsTr("Decline"), icon: Icons.callEnd, tint: Theme.tintRed, keys: "Esc",
                            run: () => PhoneCall.end() })
            } else if (home.callHere && PhoneCall.canEnd) {
                list.push({ label: qsTr("End call"), icon: Icons.callEnd, tint: Theme.tintRed, run: () => PhoneCall.end() })
            }
            list.push(
                { label: qsTr("Mirror screen"), icon: Icons.mirror, tint: Theme.tintLavender, keys: "Ctrl M",
                  enabled: home.usable("mirroring.view"), run: () => home.mirror() },
                { label: qsTr("Open an app in a window"), icon: Icons.apps, tint: Theme.tintPink,
                  enabled: home.feature("mirroring.app_windows").state === "available",
                  run: () => appsSheet.openFor(home.deviceId, home.name) },
                { label: qsTr("Send files"), icon: Icons.send, tint: Theme.tintAmber,
                  enabled: home.usable("files.send", true), run: () => filePicker.open() },
                { label: qsTr("Send a folder"), icon: Icons.folder, tint: Theme.tintAmber,
                  enabled: home.usable("files.send", true), run: () => folderPicker.open() },
                { label: qsTr("Send a file to open on arrival"), icon: Icons.openExternal, tint: Theme.tintAmber,
                  enabled: home.usable("files.send", true), run: () => handoffFilePicker.open() },
                { label: qsTr("Send clipboard"), icon: Icons.clipboard, tint: Theme.tintCoral,
                  enabled: home.usable("clipboard.pc_to_phone", true), run: () => AppController.sendClipboard(home.deviceId) },
                { label: qsTr("Clipboard history"), icon: Icons.clipboardList, tint: Theme.tintCoral,
                  enabled: Preferences.clipboardHistory, run: () => home.openClipboardHistory() },
                { label: qsTr("Take photo with phone"), icon: Icons.camera, tint: Theme.tintBlue,
                  keys: page.keycap(Preferences.continuityPhotoHotkey),
                  enabled: home.usable("camera.continuity", true),
                  run: () => AppController.startContinuityCamera("photo") },
                AppController.continuityCameraBusy
                    ? { label: qsTr("Cancel photo"), icon: Icons.close, tint: Theme.tintBlue,
                        run: () => AppController.cancelContinuityCamera() }
                    : { label: qsTr("Scan document with phone"), icon: Icons.scan, tint: Theme.tintBlue,
                        keys: page.keycap(Preferences.continuityScanHotkey),
                        enabled: home.usable("camera.continuity", true),
                        run: () => AppController.startContinuityCamera("scan") },
                { label: home.webcamHere ? qsTr("Stop webcam") : qsTr("Phone as webcam"), icon: Icons.video,
                  tint: Theme.tintCoral, checked: home.webcamHere,
                  enabled: home.webcamHere || home.usable("camera.webcam", false, true), run: () => home.toggleWebcam() },
                { label: home.typingHere ? qsTr("Stop typing on phone") : qsTr("Type on phone"), icon: Icons.keyboard,
                  tint: Theme.tintPink, checked: home.typingHere, enabled: home.online, run: () => home.toggleTyping() },
                { label: home.ringing ? qsTr("Stop ringing") : qsTr("Find phone"), icon: Icons.ring, tint: Theme.tintRed,
                  enabled: home.usable("device.find_phone"), run: () => home.findPhone() },
                { label: qsTr("Phone sound on this PC"), icon: Icons.music, tint: Theme.tintPink,
                  checked: home.phoneSound !== "off",
                  enabled: home.phoneSound !== "off" || (home.online && home.feature("mirroring.sound").state === "available"),
                  run: () => MediaList.setPhoneSound(home.deviceId, home.phoneSound === "off") },
                { label: qsTr("Phone as speaker"), icon: Icons.speaker, tint: Theme.tintTeal,
                  checked: Preferences.streamPcAudioToPhone,
                  run: () => { Preferences.streamPcAudioToPhone = !Preferences.streamPcAudioToPhone } },
                { label: qsTr("Play on phone"), icon: Icons.video, tint: Theme.tintBlue,
                  checked: MediaList.playOnPhoneState !== "off",
                  enabled: MediaList.playOnPhoneState !== "off" || MediaList.canPlayOnPhone,
                  run: () => MediaList.setPlayOnPhone(MediaList.playOnPhoneState === "off") },
                { label: AppController.notificationsPaused ? qsTr("Resume pop-ups") : qsTr("Pause pop-ups"),
                  icon: Icons.bell, tint: Theme.tintBlue, run: () => AppController.toggleNotificationsPaused() },
                { label: qsTr("Clear all notifications"), icon: Icons.close, tint: Theme.surfaceContentVariant,
                  enabled: feed.count > 0, run: () => NotificationList.dismissAll(home.deviceId) },
                { label: qsTr("Notification history"), icon: Icons.history, tint: Theme.tintBlue,
                  enabled: NotificationList.historyEnabled && NotificationHistory.count > 0,
                  run: () => home.openNotificationHistory() },
                { label: qsTr("Sync now"), icon: Icons.refresh, tint: Theme.tintTeal, run: () => AppController.syncNow() },
                { label: home.hasNickname ? qsTr("Change nickname") : qsTr("Set nickname"), icon: Icons.edit,
                  tint: Theme.surfaceContentVariant,
                  run: () => nicknameSheet.openFor(home.deviceId, home.name, home.rawName) },
                { label: qsTr("Check the connection"), icon: Icons.doctor, tint: Theme.tintTeal,
                  run: () => AppController.runDoctor() }
            )
            if (home.reconnect)
                list.push({ label: home.online ? qsTr("Disconnect") : qsTr("Stop reconnecting"), icon: Icons.unlink,
                            tint: Theme.surfaceContentVariant, run: () => AppController.setReconnect(home.deviceId, false) })
            else
                list.push({ label: qsTr("Reconnect"), icon: Icons.refresh, tint: Theme.tintTeal,
                            run: () => AppController.setReconnect(home.deviceId, true) })
            if (sendQueueModel.count > 0) {
                list.push({ label: qsTr("Cancel everything waiting to send"), icon: Icons.close,
                            tint: Theme.surfaceContentVariant, enabled: TransferList.waitingCount > 0,
                            run: () => TransferList.cancelAllQueued("") })
                list.push({ label: qsTr("Clear failed sends"), icon: Icons.close, tint: Theme.surfaceContentVariant,
                            enabled: TransferList.hasFailedOrExpiredQueue, run: () => TransferList.clearFinishedQueue() })
            }
            return list
        }

        // ---- Keys this screen advertises ----
        Shortcut {
            sequence: "Ctrl+M"
            enabled: page.active && home.visible && home.usable("mirroring.view")
            onActivated: home.mirror()
        }
        Shortcut {
            sequences: ["Ctrl+Return", "Ctrl+Enter"]
            enabled: page.active && home.visible && home.ringingHere
            onActivated: PhoneCall.answer()
        }
        Shortcut {
            sequence: "Esc"
            enabled: page.active && home.visible && home.ringingHere && !phoneControls.confirmWifiOff
                     && !appSheet.opened && !nicknameSheet.opened && !appsSheet.opened && !historySheet.opened
                     && !clipboardSheet.opened && !imagePreviewSheet.opened && !remoteKeyCapture.activeFocus
                     // Not while typing (a reply, a search): Esc clears that.
                     && !(home.Window.activeFocusItem && home.Window.activeFocusItem.cursorVisible !== undefined)
            onActivated: PhoneCall.end()
        }

        FileDialog {
            id: filePicker
            title: qsTr("Send to %1").arg(home.name)
            fileMode: FileDialog.OpenFiles
            onAccepted: TransferList.send(home.deviceId, selectedFiles.map(url => url.toString()))
        }
        FileDialog {
            id: handoffFilePicker
            title: qsTr("Hand off & open on %1").arg(home.name)
            fileMode: FileDialog.OpenFile
            onAccepted: TransferList.sendHandoff(home.deviceId, [selectedFile.toString()])
        }
        FolderDialog {
            id: folderPicker
            title: qsTr("Send a folder to %1").arg(home.name)
            onAccepted: TransferList.send(home.deviceId, [selectedFolder.toString()])
        }
        Connections {
            target: AppController
            enabled: home.visible
            function onFilePickerRequested() {
                filePicker.open()
            }
            function onHandoffFilePickerRequested() {
                handoffFilePicker.open()
            }
        }

        readonly property real sideWidth: Math.round(Math.min(Theme.columnWidth, Math.max(240, width * 0.36)))

        // ================= Left: notifications and actions =================
        Flickable {
            id: leftScroll
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: parent.width - home.sideWidth - 1
            contentHeight: leftCol.height + 24
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            Column {
                id: leftCol
                x: 8
                y: 8
                width: leftScroll.width - 16
                spacing: 2

                // ---- Notices: what's new, and an invitation to try Hive ----
                ActionRow {
                    width: parent.width
                    visible: Preferences.walkthroughCompleted && !Preferences.whatsNewDismissed
                    iconPath: Icons.book
                    tint: Theme.tintBlue
                    title: qsTr("Features guide")
                    detail: qsTr("New")
                    onClicked: {
                        Preferences.whatsNewDismissed = true
                        AppController.currentPage = "guide"
                    }
                    TextAction {
                        text: qsTr("Dismiss")
                        onClicked: Preferences.whatsNewDismissed = true
                    }
                }
                ActionRow {
                    id: hiveInvite
                    width: parent.width
                    visible: Preferences.walkthroughCompleted && Preferences.whatsNewDismissed
                             && !Preferences.hiveInviteDismissed && !Theme.hive && !Theme.quiet
                    iconPath: Icons.sparkle
                    tint: Theme.tintAmber
                    title: qsTr("Try Hive")
                    detail: qsTr("Honey on charcoal, with a living honeycomb")
                    onClicked: {
                        Preferences.hiveInviteDismissed = true
                        Preferences.theme = "hive"
                    }
                    TextAction {
                        text: qsTr("Not now")
                        onClicked: Preferences.hiveInviteDismissed = true
                    }
                }

                // ---- Notifications ----
                Item {
                    id: feed
                    width: parent.width
                    height: 30
                    readonly property var state: home.feature("notifications.mirror")
                    readonly property int count: NotificationList.count >= 0 ? NotificationList.countFor(home.deviceId) : 0
                    Txt {
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Notifications")
                        role: "label"
                        muted: true
                    }
                    Row {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        TextAction {
                            visible: NotificationList.historyEnabled && NotificationHistory.count > 0
                            text: qsTr("History")
                            Accessible.description: qsTr("Notification history (last 24 hours)")
                            onClicked: home.openNotificationHistory()
                        }
                        TextAction {
                            text: AppController.notificationsPaused ? qsTr("Resume pop-ups") : qsTr("Pause pop-ups")
                            selected: AppController.notificationsPaused
                            onClicked: AppController.toggleNotificationsPaused()
                        }
                        TextAction {
                            visible: feed.count > 0
                            text: qsTr("Clear all")
                            onClicked: NotificationList.dismissAll(home.deviceId)
                        }
                    }
                }
                // Undo, after dismissing a notification or an app's group.
                ActionRow {
                    width: parent.width
                    visible: NotificationList.undoLabel.length > 0
                    interactive: false
                    iconPath: Icons.check
                    tint: Theme.surfaceContentVariant
                    title: NotificationList.undoLabel
                    Timer {
                        id: undoTimer
                        interval: 6000
                        running: NotificationList.undoLabel.length > 0
                        onTriggered: NotificationList.clearUndo()
                    }
                    Connections {
                        target: NotificationList
                        function onUndoLabelChanged() {
                            if (NotificationList.undoLabel.length > 0)
                                undoTimer.restart()
                        }
                    }
                    TextAction {
                        text: qsTr("Undo")
                        onClicked: NotificationList.undoDismiss()
                    }
                    IconButton {
                        width: 24; height: 24
                        iconSize: 14
                        iconPath: Icons.close
                        label: qsTr("Dismiss")
                        onClicked: NotificationList.clearUndo()
                    }
                }
                // The desktop hides every app's notifications when they're
                // off or Do Not Disturb is on: say so.
                ActionRow {
                    width: parent.width
                    visible: !AppController.toastsEnabled && feed.state.state === "available"
                    interactive: false
                    iconPath: Icons.bell
                    tint: Theme.tintAmber
                    title: qsTr("Desktop pop-ups are off")
                    detail: qsTr("Turn them on in the system's notification settings")
                }
                // Ticks every minute so "5 min" ages without new events.
                Timer {
                    id: feedClock
                    property int minutes: 0
                    interval: 60000
                    repeat: true
                    running: feed.count > 0
                    onTriggered: minutes++
                }
                Column {
                    id: list
                    width: parent.width
                    spacing: 2
                    Repeater {
                        model: NotificationList
                        delegate: NotificationItem {
                            width: list.width
                            clock: feedClock.minutes
                            visible: deviceId === home.deviceId
                            height: visible ? implicitHeight : 0
                            selected: key === page.selectedNotification
                            canOpenApp: home.feature("mirroring.app_windows").state === "available" && app.length > 0
                            onSelectRequested: page.selectedNotification = selected ? "" : key
                            onOpenAppRequested: Mirror.startApp(deviceId, app, appName.length > 0 ? appName : app)
                            onOptionsRequested: appSheet.openFor(app, appName)
                            onImageClicked: (url) => imagePreviewSheet.openFor(url, title.length > 0 ? title : appName)
                        }
                    }
                }
                // Empty: say why, and what turns it on.
                Item {
                    width: parent.width
                    height: Theme.rowHeight
                    visible: feed.count === 0
                    Txt {
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !lockedHint.visible
                        muted: true
                        text: feed.state.state === "available"
                              ? qsTr("No notifications")
                              : home.lastSeen > 0 || home.online
                                ? qsTr("Notifications unavailable")
                                : qsTr("Connect %1 to see notifications").arg(home.name)
                    }
                    LockChip {
                        id: lockedHint
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        visible: label.length > 0
                        maxWidth: 260
                        feature: feed.state
                    }
                }

                // ---- Actions ----
                SectionLabel {
                    width: parent.width
                    text: qsTr("Actions")
                }
                PhoneAction {
                    deviceOnline: home.online
                    title: qsTr("Mirror screen")
                    iconPath: Icons.mirror
                    glyphTint: Theme.tintLavender
                    keys: "Ctrl M"
                    feature: home.feature("mirroring.view")
                    active: home.mirroring
                    meta: home.mirroring ? qsTr("On") : ""
                    onClicked: home.mirror()
                    IconButton {
                        visible: home.feature("mirroring.app_windows").state === "available"
                        width: 26; height: 26
                        iconSize: 14
                        iconPath: Icons.apps
                        label: qsTr("Open an app in a window")
                        onClicked: appsSheet.openFor(home.deviceId, home.name)
                    }
                }
                PhoneAction {
                    id: sendFilesAction
                    deviceOnline: home.online
                    title: qsTr("Send files")
                    iconPath: Icons.send
                    glyphTint: Theme.tintAmber
                    queueable: true
                    feature: home.feature("files.send")
                    onClicked: filePicker.open()
                    IconButton {
                        visible: sendFilesAction.available
                        width: 26; height: 26
                        iconSize: 14
                        iconPath: Icons.folder
                        label: qsTr("Send a folder")
                        onClicked: folderPicker.open()
                    }
                }
                PhoneAction {
                    deviceOnline: home.online
                    title: qsTr("Send clipboard")
                    iconPath: Icons.clipboard
                    glyphTint: Theme.tintCoral
                    queueable: true
                    feature: home.feature("clipboard.pc_to_phone")
                    onClicked: AppController.sendClipboard(home.deviceId)
                    IconButton {
                        visible: Preferences.clipboardHistory
                        width: 26; height: 26
                        iconSize: 14
                        iconPath: Icons.clipboardList
                        label: qsTr("Clipboard history")
                        onClicked: home.openClipboardHistory()
                    }
                }
                PhoneAction {
                    deviceOnline: home.online
                    title: qsTr("Take photo with phone")
                    iconPath: Icons.camera
                    glyphTint: Theme.tintBlue
                    keys: page.keycap(Preferences.continuityPhotoHotkey)
                    queueable: true
                    feature: home.feature("camera.continuity")
                    active: AppController.continuityCameraBusy
                    detail: AppController.continuityCameraBusy ? AppController.continuityCameraStatus : ""
                    onClicked: AppController.startContinuityCamera("photo")
                    IconButton {
                        width: 26; height: 26
                        iconSize: 14
                        iconPath: AppController.continuityCameraBusy ? Icons.close : Icons.scan
                        label: AppController.continuityCameraBusy ? qsTr("Cancel") : qsTr("Scan document")
                        onClicked: {
                            if (AppController.continuityCameraBusy)
                                AppController.cancelContinuityCamera()
                            else
                                AppController.startContinuityCamera("scan")
                        }
                    }
                }
                PhoneAction {
                    deviceOnline: home.online
                    title: home.webcamHere ? qsTr("Stop webcam") : qsTr("Phone as webcam")
                    iconPath: Icons.video
                    glyphTint: Theme.tintCoral
                    addonOk: true
                    feature: home.feature("camera.webcam")
                    active: home.webcamHere
                    forceAvailable: home.webcamHere
                    detail: home.webcamHere ? (Webcam.statusText.length > 0 ? Webcam.statusText : qsTr("Streaming")) : ""
                    onClicked: home.toggleWebcam()
                }
                PhoneAction {
                    id: typeAction
                    deviceOnline: home.online
                    title: home.typingHere ? qsTr("Stop typing on phone") : qsTr("Type on phone")
                    iconPath: Icons.keyboard
                    glyphTint: Theme.tintPink
                    feature: home.feature("mirroring.control")
                    // Starts whenever the phone is connected; the phone says
                    // if it needs more.
                    forceAvailable: home.online
                    active: home.typingHere
                    detail: home.typingHere && typeBox.lastSent.length > 0 ? qsTr("Sent: %1").arg(typeBox.lastSent) : ""
                    onClicked: home.toggleTyping()
                    LockChip {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: home.online && typeAction.feature.state !== "available" && label.length > 0
                        feature: typeAction.feature
                        maxWidth: 200
                    }
                }
                // While typing: the box the keys go through.
                Rectangle {
                    id: remoteKeyCapture
                    width: parent.width
                    height: home.typingHere ? 40 : 0
                    visible: height > 0
                    radius: Theme.rowRadius
                    color: Theme.raised
                    border.width: activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                    activeFocusOnTab: home.typingHere
                    // Screen readers can find and focus the box the keys go through.
                    Accessible.role: Accessible.EditableText
                    Accessible.name: qsTr("Type on phone")
                    Accessible.focusable: true
                    onVisibleChanged: if (visible) forceActiveFocus()

                    QtObject {
                        id: typeBox
                        property string lastSent: ""
                        // What's been typed lately, as it would read rather than key by key.
                        property string typed: ""
                        function sentText(text) {
                            typed = (typed + text).slice(-40)
                            lastSent = typed
                        }
                        function sentKey(name) {
                            if (name === "backspace" && typed.length > 0) {
                                const chars = Array.from(typed)
                                chars.pop()
                                typed = chars.join("")
                                if (typed.length > 0) {
                                    lastSent = typed
                                    return
                                }
                            }
                            typed = ""
                            lastSent = name
                        }
                    }
                    Connections {
                        target: home
                        function onTypingHereChanged() {
                            if (!home.typingHere) {
                                typeBox.lastSent = ""
                                typeBox.typed = ""
                            }
                        }
                    }

                    Keys.onPressed: (event) => {
                        if (event.key === Qt.Key_Escape) {
                            Mirror.stopKeyboard()
                            event.accepted = true
                            return
                        }
                        if ((event.modifiers & Qt.ControlModifier) && event.key === Qt.Key_V) {
                            Mirror.keyboardPaste(home.deviceId)
                            typeBox.lastSent = qsTr("Clipboard")
                            event.accepted = true
                            return
                        }
                        let keyName = ""
                        switch (event.key) {
                        case Qt.Key_Return:
                        case Qt.Key_Enter: keyName = "enter"; break
                        case Qt.Key_Backspace: keyName = "backspace"; break
                        case Qt.Key_Delete: keyName = "delete"; break
                        case Qt.Key_Left: keyName = "left"; break
                        case Qt.Key_Right: keyName = "right"; break
                        case Qt.Key_Up: keyName = "up"; break
                        case Qt.Key_Down: keyName = "down"; break
                        case Qt.Key_Tab: keyName = "tab"; break
                        default: break
                        }
                        if (keyName.length > 0) {
                            Mirror.keyboardPress(home.deviceId, keyName)
                            typeBox.sentKey(keyName)
                            event.accepted = true
                            return
                        }
                        const edit = (event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.ShiftModifier | Qt.MetaModifier))
                            === Qt.ControlModifier
                            ? ({ [Qt.Key_C]: "copy", [Qt.Key_X]: "cut", [Qt.Key_A]: "select_all" })[event.key] : undefined
                        if (edit !== undefined) {
                            Mirror.keyboardPress(home.deviceId, edit)
                            event.accepted = true
                            return
                        }
                        if (event.text && event.text.length > 0 && event.text.charCodeAt(0) >= 32
                                && (!(event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier))
                                    // AltGr (@ on a French keyboard, say) can arrive as Ctrl+Alt.
                                    // A plain letter, digit or space with Ctrl+Alt is a shortcut:
                                    // Qt gives Ctrl+Alt+2 the text "2" even without AltGr.
                                    || ((event.modifiers & Qt.ControlModifier) && (event.modifiers & Qt.AltModifier)
                                        && !/^[A-Za-z0-9 ]$/.test(event.text)))) {
                            Mirror.keyboardText(home.deviceId, event.text)
                            typeBox.sentText(event.text)
                            event.accepted = true
                        }
                    }

                    Txt {
                        x: Theme.rowPadding
                        anchors.right: pasteBtn.left
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        color: remoteKeyCapture.activeFocus ? Theme.surfaceContent : Theme.surfaceContentVariant
                        text: remoteKeyCapture.activeFocus
                            ? qsTr("Keys go to %1 · Ctrl V pastes · Esc stops").arg(home.name)
                            : qsTr("Click to type")
                    }
                    TextAction {
                        id: pasteBtn
                        anchors.right: parent.right
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Paste")
                        onClicked: {
                            Mirror.keyboardPaste(home.deviceId)
                            typeBox.lastSent = qsTr("Clipboard")
                            remoteKeyCapture.forceActiveFocus()
                        }
                    }
                    TapHandler {
                        onTapped: remoteKeyCapture.forceActiveFocus()
                    }
                }
                PhoneAction {
                    deviceOnline: home.online
                    title: home.ringing ? qsTr("Stop ringing") : qsTr("Find phone")
                    iconPath: Icons.ring
                    glyphTint: Theme.tintRed
                    feature: home.feature("device.find_phone")
                    active: home.ringing
                    onClicked: home.findPhone()
                }

                // ---- Waiting to send ----
                Item {
                    width: parent.width
                    height: 30
                    visible: sendQueueModel.count > 0
                    Txt {
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        role: "label"
                        muted: true
                        text: page.sendQueueWaiting > 0
                              ? qsTr("Waiting to send · %1").arg(page.sendQueueWaiting)
                              : qsTr("Waiting to send")
                    }
                    Row {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        TextAction {
                            visible: TransferList.hasFailedOrExpiredQueue
                            text: qsTr("Clear")
                            onClicked: TransferList.clearFinishedQueue()
                        }
                        TextAction {
                            visible: TransferList.waitingCount > 0
                            text: qsTr("Cancel all")
                            onClicked: TransferList.cancelAllQueued("")
                        }
                    }
                }
                Repeater {
                    model: sendQueueModel
                    delegate: ActionRow {
                        id: queueRow
                        required property var model
                        readonly property string itemStatus: queueRow.model.status || "waiting"
                        readonly property bool isFailedOrExpired: itemStatus === "failed" || itemStatus === "expired"
                        readonly property real totalBytes: Number(queueRow.model.totalBytes || 0)
                        readonly property real progress: Number(queueRow.model.progress || 0)
                        width: leftCol.width
                        interactive: false
                        iconPath: queueRow.model.icon === "folder" ? Icons.folder
                            : queueRow.model.icon === "clipboard" ? Icons.clipboard
                            : queueRow.model.icon === "link" ? Icons.globe
                            : queueRow.model.icon === "task" ? Icons.bolt
                            : queueRow.model.icon === "camera" ? Icons.camera
                            : Icons.send
                        tint: isFailedOrExpired ? Theme.tintRed : Theme.tintAmber
                        title: queueRow.model.title || ""
                        detail: {
                            const devName = (queueRow.model.deviceName && queueRow.model.deviceName.length > 0)
                                ? queueRow.model.deviceName : home.name
                            const sz = page.formatQueueSize(queueRow.totalBytes)
                            if (queueRow.itemStatus === "sending") {
                                if (queueRow.totalBytes > 0) {
                                    const pct = Math.round(Math.max(0, Math.min(1, queueRow.progress)) * 100)
                                    return qsTr("Sending to %1 · %2%").arg(devName).arg(pct)
                                }
                                return qsTr("Sending to %1…").arg(devName)
                            }
                            if (queueRow.itemStatus === "failed") {
                                const reason = queueRow.model.reason || ""
                                return reason.length > 0 ? qsTr("Failed · %1").arg(reason) : qsTr("Failed")
                            }
                            if (queueRow.itemStatus === "expired") {
                                const reason = queueRow.model.reason || ""
                                return reason.length > 0 ? reason : qsTr("Expired")
                            }
                            return sz.length > 0
                                ? qsTr("Waiting for %1 · %2").arg(devName).arg(sz)
                                : qsTr("Waiting for %1").arg(devName)
                        }
                        IconButton {
                            visible: queueRow.isFailedOrExpired
                            width: 26; height: 26
                            iconSize: 14
                            iconPath: Icons.refresh
                            label: qsTr("Retry")
                            onClicked: TransferList.retryQueueItem(queueRow.model.queueId)
                        }
                        IconButton {
                            width: 26; height: 26
                            iconSize: 14
                            iconPath: Icons.close
                            label: queueRow.isFailedOrExpired ? qsTr("Dismiss") : qsTr("Cancel")
                            onClicked: TransferList.cancelQueueItem(queueRow.model.queueId)
                        }
                    }
                }

                // ---- Transfers ----
                Item {
                    width: parent.width
                    height: 30
                    visible: TransferList.count > 0
                    Txt {
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        role: "label"
                        muted: true
                        text: qsTr("Transfers")
                    }
                    Row {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        TextAction {
                            text: qsTr("Timeline")
                            onClicked: AppController.currentPage = "timeline"
                        }
                        TextAction {
                            visible: TransferList.count > TransferList.active
                            text: qsTr("Clear finished")
                            onClicked: TransferList.clearFinished()
                        }
                    }
                }
                Repeater {
                    model: TransferList
                    delegate: TransferItem { width: leftCol.width }
                }

                // ---- LocalSend nearby ----
                LocalSendSection {
                    width: parent.width
                    visible: AppController.localsendEnabled && page.localsendPeers.length > 0
                }

                // ---- Timeline: the latest across phones ----
                Item {
                    id: timelineHeader
                    width: parent.width
                    height: 30
                    readonly property var previewItems: {
                        try {
                            return AppController.timelinePreview.length > 0
                                ? JSON.parse(AppController.timelinePreview) : []
                        } catch (e) {
                            return []
                        }
                    }
                    Txt {
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        role: "label"
                        muted: true
                        text: qsTr("Recent")
                    }
                    TextAction {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Timeline")
                        onClicked: AppController.currentPage = "timeline"
                    }
                }
                Txt {
                    x: Theme.rowPadding
                    height: Theme.rowHeight
                    visible: timelineHeader.previewItems.length === 0
                    muted: true
                    text: qsTr("Nothing yet")
                }
                Repeater {
                    model: timelineHeader.previewItems
                    delegate: ActionRow {
                        required property var modelData
                        width: leftCol.width
                        iconPath: modelData.kind === "file" ? Icons.folder
                            : modelData.kind === "clip" ? Icons.clipboard
                            : modelData.kind === "link" ? Icons.globe
                            : modelData.kind === "photo" ? Icons.photo
                            : modelData.kind === "recording" ? Icons.mic
                            : Icons.mirror
                        tint: modelData.kind === "file" ? Theme.tintAmber
                            : modelData.kind === "clip" ? Theme.tintLavender
                            : modelData.kind === "link" ? Theme.tintBlue
                            : modelData.kind === "photo" ? Theme.tintCoral
                            : modelData.kind === "recording" ? Theme.tintRed
                            : Theme.tintPink
                        title: modelData.title || ""
                        detail: modelData.subtitle || ""
                        onClicked: AppController.currentPage = "timeline"
                    }
                }
            }
        }

        Rectangle {
            x: leftScroll.width
            width: 1
            height: parent.height
            color: Theme.line
        }

        // ================= Right: the phone =================
        Flickable {
            id: sideScroll
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: home.sideWidth
            contentHeight: side.height + 32
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            Column {
                id: side
                x: 16
                y: 16
                width: sideScroll.width - 32
                spacing: 14

                // The call on the phone: ringing or in progress.
                Binding { target: PhoneCall; property: "device"; value: home.deviceId; when: home.visible; restoreMode: Binding.RestoreNone }
                CallCard {
                    width: parent.width
                    visible: home.callHere
                }

                // ---- The phone: name, Android, network ----
                Item {
                    width: parent.width
                    height: Math.max(phoneGlyph.y + phoneGlyph.height, identity.y + identity.implicitHeight)
                    // The glyph sits on the name's line, not between the lines under it.
                    Icon {
                        id: phoneGlyph
                        y: identity.y + Math.round((identityName.height - phoneGlyph.height) / 2)
                        width: 28; height: 28
                        path: home.kind === "phone" || home.kind === "tablet" ? Icons.phone : Icons.laptop
                        color: home.online ? Theme.accent : Theme.surfaceContentVariant
                    }
                    Column {
                        id: identity
                        anchors.left: phoneGlyph.right
                        anchors.leftMargin: 12
                        anchors.right: parent.right
                        y: Math.max(0, Math.round((phoneGlyph.height - identityName.height) / 2))
                        Row {
                            id: identityName
                            width: parent.width
                            spacing: 4
                            Txt {
                                id: nameTxt
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.min(implicitWidth, parent.width - editNickname.width - 4)
                                text: home.name
                                weight: 500
                            }
                            IconButton {
                                id: editNickname
                                anchors.verticalCenter: parent.verticalCenter
                                width: 24; height: 24
                                iconSize: 12
                                iconPath: Icons.edit
                                label: home.hasNickname ? qsTr("Change nickname") : qsTr("Set nickname")
                                onClicked: nicknameSheet.openFor(home.deviceId, home.name, home.rawName)
                            }
                        }
                        Txt {
                            width: parent.width
                            visible: text.length > 0
                            role: "bodySmall"
                            muted: true
                            // The model is often a code (like "SM-X000"), so only the version.
                            text: [home.osVersion.length > 0 ? qsTr("Android %1").arg(home.osVersion) : "",
                                   home.cellType].filter(s => s.length > 0).join(" · ")
                        }
                    }
                }

                // ---- The link ----
                Flow {
                    width: parent.width
                    spacing: 4
                    TextAction {
                        iconPath: !home.online ? Icons.unlink
                                  : home.path === "usb" ? Icons.usb
                                  : home.path === "relay" ? Icons.globe
                                  : (home.wifiBars < 0 && home.cellBars >= 0 ? Icons.signal : Icons.wifi)
                        iconColor: home.online ? Theme.tintTeal : Theme.surfaceContentVariant
                        Accessible.description: qsTr("Check the connection")
                        text: home.online
                              ? (home.path === "relay"
                                 ? (home.rttMs >= 1 ? qsTr("Away · %1 ms").arg(home.rttMs) : qsTr("Away"))
                                 : (home.path === "usb"
                                    ? (home.rttMs >= 1 ? qsTr("USB · %1 ms").arg(home.rttMs) : qsTr("USB"))
                                    : (home.wifiBars >= 0
                                       ? (home.rttMs >= 1 ? qsTr("Wi‑Fi · %1 ms").arg(home.rttMs) : qsTr("Wi‑Fi"))
                                       : (home.cellBars >= 0
                                          ? ((home.cellType.length > 0 ? home.cellType : qsTr("Cellular"))
                                             + (home.rttMs >= 1 ? qsTr(" · %1 ms").arg(home.rttMs) : ""))
                                          : (home.rttMs >= 1 ? qsTr("Wi‑Fi · %1 ms").arg(home.rttMs) : qsTr("Wi‑Fi"))))))
                              : !home.reconnect ? qsTr("Not reconnecting")
                              : (home.lastSeen > 0 ? qsTr("Offline · %1").arg(page.relativeTime(home.lastSeen))
                                                   : qsTr("Offline"))
                        onClicked: AppController.runDoctor()
                    }
                    // Stays paired, and stays disconnected until Reconnect (GitHub #48).
                    TextAction {
                        visible: home.reconnect && home.online
                        text: qsTr("Disconnect")
                        onClicked: AppController.setReconnect(home.deviceId, false)
                    }
                    TextAction {
                        visible: home.reconnect && !home.online
                        text: qsTr("Stop reconnecting")
                        onClicked: AppController.setReconnect(home.deviceId, false)
                    }
                    TextAction {
                        visible: !home.online
                        text: home.reconnect ? qsTr("Troubleshoot") : qsTr("Reconnect")
                        onClicked: home.reconnect ? AppController.runDoctor()
                                                  : AppController.setReconnect(home.deviceId, true)
                    }
                }

                // ---- Battery ----
                Column {
                    width: parent.width
                    spacing: 6
                    visible: home.battery >= 0
                    Item {
                        width: parent.width
                        height: batteryText.height
                        Txt {
                            role: "bodySmall"
                            muted: true
                            text: qsTr("Battery")
                        }
                        Txt {
                            id: batteryText
                            anchors.right: parent.right
                            role: "bodySmall"
                            color: Theme.batteryColor(home.battery)
                            text: home.charging ? qsTr("%1% · charging").arg(home.battery) : qsTr("%1%").arg(home.battery)
                        }
                    }
                    Rectangle {
                        width: parent.width
                        height: 4
                        radius: 2
                        color: Theme.selection
                        Rectangle {
                            width: parent.width * Math.max(0, Math.min(100, home.battery)) / 100
                            height: parent.height
                            radius: 2
                            color: Theme.batteryColor(home.battery)
                        }
                    }
                    Txt {
                        width: parent.width
                        visible: text.length > 0
                        role: "bodySmall"
                        muted: true
                        text: home.charging ? page.formatChargeEta(home.batteryFullIn) : ""
                    }
                }

                // ---- The webcam, while it streams ----
                Column {
                    id: webcamHomeCard
                    width: parent.width
                    spacing: 8
                    visible: home.webcamHere
                    readonly property bool previewWanted: home.webcamHere && visible && home.visible
                        && webcamHomeCard.Window.window !== null && webcamHomeCard.Window.window.visible
                        && webcamHomeCard.Window.window.visibility !== Window.Minimized
                    onPreviewWantedChanged: Webcam.setPreviewVisible("home", previewWanted)
                    Component.onCompleted: Webcam.setPreviewVisible("home", previewWanted)
                    Component.onDestruction: Webcam.setPreviewVisible("home", false)
                    Item {
                        width: parent.width
                        height: 24
                        Txt {
                            anchors.left: parent.left
                            anchors.right: webcamStop.left
                            anchors.verticalCenter: parent.verticalCenter
                            role: "label"
                            muted: true
                            text: Webcam.statusText.length > 0 ? Webcam.statusText : qsTr("Webcam")
                        }
                        TextAction {
                            id: webcamStop
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Stop")
                            onClicked: Webcam.stop()
                        }
                    }
                    Rectangle {
                        width: parent.width
                        height: Math.round(width * 9 / 16)
                        radius: Theme.rowRadius
                        color: "#0e0e10"
                        clip: true
                        VideoView {
                            anchors.fill: parent
                            stream: "webcam"
                        }
                    }
                    Flow {
                        width: parent.width
                        spacing: 4
                        Segmented {
                            options: [
                                { value: "back", label: qsTr("Back") },
                                { value: "front", label: qsTr("Front") }
                            ]
                            value: Webcam.camera
                            onPicked: (v) => Webcam.setCamera(v)
                        }
                        Segmented {
                            // 4K only when the phone's camera in use can stream it.
                            options: [
                                { value: "720", label: "720p" },
                                { value: "1080", label: "1080p" }
                            ].concat(Webcam.uhd ? [{ value: "2160", label: "4K" }] : [])
                            value: String(Webcam.height)
                            onPicked: (v) => Webcam.setResolution(Number(v))
                        }
                        TextAction {
                            text: qsTr("Mirror")
                            selected: Webcam.mirror
                            onClicked: Webcam.setMirrorImage(!Webcam.mirror)
                        }
                    }
                }

                // ---- Phone controls ----
                Column {
                    id: phoneControls
                    width: parent.width
                    spacing: 6
                    visible: home.online && state !== null
                    property bool expanded: false
                    property bool confirmWifiOff: false
                    property bool hotspotActive: false
                    property var hoveredLockedFeature: null
                    // The phone doesn't report its hotspot, so this is only ever what was asked.
                    Connections {
                        target: AppController
                        function onPhoneToggleFailed(device, id) {
                            if (device === home.deviceId && id === "hotspot")
                                phoneControls.hotspotActive = !phoneControls.hotspotActive
                        }
                    }
                    readonly property var state: {
                        if (AppController.phoneTogglesRevision < 0)
                            return null
                        const raw = AppController.phoneToggles(home.deviceId)
                        if (!raw || raw.length === 0)
                            return null
                        try {
                            return JSON.parse(raw)
                        } catch (e) {
                            return null
                        }
                    }
                    readonly property bool hasFlashlight: state !== null
                        && state.flashlight !== undefined && state.flashlight !== null
                    readonly property var activeLockFeature: {
                        if (hoveredLockedFeature && hoveredLockedFeature.state === "locked")
                            return hoveredLockedFeature
                        return ({})
                    }
                    function showLock(f) { hoveredLockedFeature = (hoveredLockedFeature === f ? ({}) : f) }
                    onVisibleChanged: if (!visible) confirmWifiOff = false

                    Shortcut {
                        sequence: "Esc"
                        enabled: page.active && home.visible && phoneControls.confirmWifiOff
                        onActivated: phoneControls.confirmWifiOff = false
                    }

                    Item {
                        width: parent.width
                        height: 24
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Phone controls")
                            role: "label"
                            muted: true
                        }
                        TextAction {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: phoneControls.expanded ? qsTr("Less") : qsTr("Sound and display")
                            onClicked: phoneControls.expanded = !phoneControls.expanded
                        }
                    }
                    Grid {
                        id: toggleGrid
                        width: parent.width
                        columns: 2
                        columnSpacing: 6
                        rowSpacing: 6
                        readonly property real cell: (width - columnSpacing) / 2
                        ToggleTile {
                            width: toggleGrid.cell
                            iconPath: Icons.moon
                            label: qsTr("DND", "Do not disturb, on a narrow tile")
                            fullName: qsTr("Do not disturb")
                            active: phoneControls.state ? Boolean(phoneControls.state.dnd) : false
                            feature: home.feature("toggles.dnd")
                            onLockedClicked: phoneControls.showLock(feature)
                            onClicked: AppController.setPhoneToggle(home.deviceId, "dnd", String(!active))
                        }
                        ToggleTile {
                            width: toggleGrid.cell
                            iconPath: Icons.wifi
                            label: qsTr("Wi‑Fi")
                            active: phoneControls.state ? Boolean(phoneControls.state.wifi) : false
                            feature: home.feature("toggles.wifi")
                            onLockedClicked: phoneControls.showLock(feature)
                            onClicked: {
                                if (active)
                                    phoneControls.confirmWifiOff = true
                                else
                                    AppController.setPhoneToggle(home.deviceId, "wifi", "true")
                            }
                        }
                        ToggleTile {
                            width: toggleGrid.cell
                            iconPath: Icons.bluetooth
                            label: qsTr("Bluetooth")
                            active: phoneControls.state ? Boolean(phoneControls.state.bluetooth) : false
                            feature: home.feature("toggles.bluetooth")
                            onLockedClicked: phoneControls.showLock(feature)
                            onClicked: AppController.setPhoneToggle(home.deviceId, "bluetooth", String(!active))
                        }
                        ToggleTile {
                            visible: phoneControls.hasFlashlight
                            width: toggleGrid.cell
                            iconPath: Icons.flashlight
                            label: qsTr("Torch")
                            active: phoneControls.state !== null && phoneControls.state.flashlight === true
                            feature: home.feature("toggles.flashlight")
                            onLockedClicked: phoneControls.showLock(feature)
                            onClicked: AppController.setPhoneToggle(home.deviceId, "flashlight", String(!active))
                        }
                        ToggleTile {
                            width: toggleGrid.cell
                            iconPath: Icons.hotspot
                            label: qsTr("Hotspot")
                            active: phoneControls.hotspotActive
                            feature: home.feature("toggles.wifi")
                            onLockedClicked: phoneControls.showLock(feature)
                            onClicked: {
                                phoneControls.hotspotActive = !phoneControls.hotspotActive
                                AppController.setPhoneToggle(home.deviceId, "hotspot", String(phoneControls.hotspotActive))
                            }
                        }
                    }
                    LockChip {
                        visible: label.length > 0
                        feature: phoneControls.activeLockFeature
                        maxWidth: parent.width
                    }
                    // Turning off Wi-Fi can cut the link: ask first.
                    Item {
                        width: parent.width
                        height: Theme.rowHeight
                        visible: phoneControls.confirmWifiOff && phoneControls.state !== null && Boolean(phoneControls.state.wifi)
                        Txt {
                            anchors.left: parent.left
                            anchors.right: wifiButtons.left
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            role: "bodySmall"
                            text: qsTr("May disconnect %1").arg(home.name)
                        }
                        Row {
                            id: wifiButtons
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 4
                            TextAction {
                                text: qsTr("Cancel")
                                onClicked: phoneControls.confirmWifiOff = false
                            }
                            TextAction {
                                text: qsTr("Turn off Wi‑Fi")
                                selected: true
                                onClicked: {
                                    phoneControls.confirmWifiOff = false
                                    AppController.setPhoneToggle(home.deviceId, "wifi", "false")
                                }
                            }
                        }
                    }

                    // Sound and display: ringer mode, volume and brightness.
                    Column {
                        width: parent.width
                        spacing: 10
                        visible: phoneControls.expanded
                        topPadding: 4
                        readonly property var ringerFeature: home.feature("toggles.ringer")
                        Segmented {
                            width: parent.width
                            enabled: parent.ringerFeature.state === "available" || parent.ringerFeature.state === "partial"
                            opacity: enabled ? 1 : 0.5
                            options: [
                                { value: "ring", label: qsTr("Ring") },
                                { value: "vibrate", label: qsTr("Vibrate") },
                                { value: "silent", label: qsTr("Silent") }
                            ]
                            value: phoneControls.state ? phoneControls.state.ringer : "ring"
                            onPicked: (mode) => AppController.setPhoneToggle(home.deviceId, "ringer", mode)
                        }
                        LockChip {
                            visible: parent.ringerFeature.state === "locked" && label.length > 0
                            feature: parent.ringerFeature
                            maxWidth: parent.width
                        }
                        ToggleSlider {
                            width: parent.width
                            deviceId: home.deviceId
                            toggleId: "volume"
                            title: qsTr("Volume")
                            iconPath: shownValue === 0 ? Icons.soundOff : Icons.speaker
                            phoneValue: phoneControls.state ? phoneControls.state.volume : 0
                            feature: home.feature("toggles.volume")
                        }
                        ToggleSlider {
                            width: parent.width
                            deviceId: home.deviceId
                            toggleId: "brightness"
                            title: qsTr("Brightness")
                            iconPath: Icons.brightness
                            phoneValue: phoneControls.state ? phoneControls.state.brightness : 0
                            feature: home.feature("toggles.brightness")
                        }
                    }
                }

                // ---- Sound between the two ----
                Column {
                    width: parent.width
                    spacing: 0
                    SwitchBar {
                        width: parent.width
                        iconPath: Icons.speaker
                        tint: Theme.tintTeal
                        title: qsTr("Phone as speaker")
                        checked: Preferences.streamPcAudioToPhone
                        onToggled: (on) => Preferences.streamPcAudioToPhone = on
                    }
                    // Play on phone: the video playing on this PC, on the phone.
                    // Greyed out until a video plays.
                    SwitchBar {
                        readonly property string watch: MediaList.playOnPhoneState
                        width: parent.width
                        iconPath: Icons.video
                        tint: Theme.tintBlue
                        title: qsTr("Play on phone")
                        enabled: watch !== "off" || MediaList.canPlayOnPhone
                        checked: watch !== "off"
                        caption: watch === "watching" ? qsTr("On %1").arg(MediaList.playOnPhoneName)
                            : watch === "asking" ? qsTr("Tap the notification on %1").arg(MediaList.playOnPhoneName)
                            : ""
                        onToggled: (on) => MediaList.setPlayOnPhone(on)
                    }
                    // The phone's sound out of this PC's speakers, without a mirror window.
                    SwitchBar {
                        width: parent.width
                        visible: home.phoneSound !== "off" || (home.online && home.feature("mirroring.sound").state === "available")
                        iconPath: Icons.music
                        tint: Theme.tintPink
                        title: qsTr("Phone sound here")
                        checked: home.phoneSound !== "off"
                        caption: home.phoneSound === "asking" ? qsTr("Allow it on %1").arg(home.name) : ""
                        onToggled: (on) => MediaList.setPhoneSound(home.deviceId, on)
                    }
                }

                // ---- Now playing (phone or this PC) ----
                QtObject {
                    id: media
                    readonly property int phoneRow: MediaList.revision >= 0 ? MediaList.firstFor(home.deviceId) : -1
                    readonly property int localRow: MediaList.revision >= 0 ? MediaList.firstFor("local") : -1
                    readonly property bool hasBoth: phoneRow >= 0 && localRow >= 0
                    readonly property bool phonePlaying: phoneRow >= 0 && MediaList.revision >= 0 && MediaList.isPlaying(phoneRow)
                    readonly property bool localPlaying: localRow >= 0 && MediaList.revision >= 0 && MediaList.isPlaying(localRow)
                    property string preferred: ""
                    onPhonePlayingChanged: {
                        if (phonePlaying && !localPlaying && preferred !== "phone")
                            Qt.callLater(() => {
                                if (media.phonePlaying && !media.localPlaying)
                                    media.preferred = "phone"
                            })
                    }
                    onLocalPlayingChanged: {
                        if (localPlaying && !phonePlaying && preferred !== "local")
                            Qt.callLater(() => {
                                if (media.localPlaying && !media.phonePlaying)
                                    media.preferred = "local"
                            })
                    }
                    readonly property int activeRow: {
                        if (preferred === "local" && localRow >= 0)
                            return localRow
                        if (preferred === "phone" && phoneRow >= 0)
                            return phoneRow
                        if (localPlaying && !phonePlaying && localRow >= 0)
                            return localRow
                        if (phonePlaying && !localPlaying && phoneRow >= 0)
                            return phoneRow
                        if (phoneRow >= 0)
                            return phoneRow
                        return localRow
                    }
                }
                Column {
                    width: parent.width
                    spacing: 8
                    Txt {
                        text: qsTr("Now playing")
                        role: "label"
                        muted: true
                    }
                    Txt {
                        visible: media.activeRow < 0
                        role: "bodySmall"
                        muted: true
                        text: qsTr("Nothing playing")
                    }
                    Repeater {
                        model: MediaList
                        delegate: NowPlaying {
                            required property int index
                            visible: MediaList.revision >= 0 && index === media.activeRow
                            width: visible ? side.width : 0
                            embeddedInHero: true
                            hasMultipleSources: media.hasBoth
                            phoneVolume: phoneControls.state ? phoneControls.state.volume : 65
                            onSourceSelected: (src) => media.preferred = src
                        }
                    }
                }
            }
        }
    }

    // What one app's notifications do on this PC.
    Sheet {
        id: appSheet
        property string app
        property string appName
        property string rule: "show"
        cardWidth: 420
        function openFor(app, name) {
            appSheet.app = app
            appSheet.appName = name
            appSheet.rule = NotificationList.appRule(app)
            open()
        }
        Column {
            width: parent.width
            spacing: 16
            Txt { width: parent.width; text: qsTr("%1 notifications").arg(appSheet.appName); role: "headline"; wrapMode: Text.WordWrap }
            Segmented {
                options: [
                    { value: "show", label: qsTr("Show") },
                    { value: "quiet", label: qsTr("No pop-ups") },
                    { value: "hidden", label: qsTr("Hide") }
                ]
                value: appSheet.rule
                onPicked: (value) => {
                    appSheet.rule = value
                    NotificationList.setAppRule(appSheet.app, value)
                }
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: appSheet.rule === "hidden"
                      ? qsTr("Hidden on this PC.")
                      : appSheet.rule === "quiet"
                        ? qsTr("Shown in feed without pop-ups.")
                        : qsTr("Shown in feed and as desktop pop-ups.")
            }
            Row {
                anchors.right: parent.right
                Button { text: qsTr("Done"); onClicked: appSheet.close() }
            }
        }
    }

    // Set a custom PC-side nickname for a phone.
    Sheet {
        id: nicknameSheet
        property string targetDeviceId: ""
        property string currentName: ""
        property string rawName: ""
        cardWidth: 420
        function openFor(devId, name, raw) {
            targetDeviceId = devId
            currentName = name
            rawName = raw
            nicknameInput.text = (name !== raw) ? name : ""
            open()
            nicknameInput.forceActiveFocus()
        }
        function saveNickname() {
            Preferences.setNickname(targetDeviceId, nicknameInput.text.trim())
            nicknameSheet.close()
        }
        Column {
            width: parent.width
            spacing: 16
            Txt {
                width: parent.width
                text: qsTr("Phone nickname")
                role: "headline"
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: qsTr("Blank uses \"%1\"").arg(nicknameSheet.rawName)
            }
            Rectangle {
                width: parent.width
                height: 40
                radius: Theme.radiusMd
                color: Theme.raised
                border.width: nicknameInput.activeFocus ? 1 : 0
                border.color: nicknameInput.activeFocus ? Theme.focusRing : Theme.outlineVariant

                TextInput {
                    id: nicknameInput
                    anchors.fill: parent
                    anchors.margins: 10
                    color: Theme.surfaceContent
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.body.size
                    selectByMouse: true
                    clip: true
                    onAccepted: nicknameSheet.saveNickname()
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 8
                Button {
                    text: qsTr("Cancel")
                    variant: "outline"
                    onClicked: nicknameSheet.close()
                }
                Button {
                    text: qsTr("Save")
                    onClicked: nicknameSheet.saveNickname()
                }
            }
        }
    }

    // A phone's apps, to open one in a window of its own.
    AppsSheet {
        id: appsSheet
    }

    // Notifications from the last day that are gone from the phone.
    Sheet {
        id: historySheet
        cardWidth: 560
        Column {
            width: parent.width
            spacing: 12
            Item {
                width: parent.width
                height: Math.max(historyTitle.height, clearHistory.height)
                Txt { id: historyTitle; anchors.verticalCenter: parent.verticalCenter; text: qsTr("Notification history"); role: "headline" }
                Button {
                    id: clearHistory
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    visible: NotificationHistory.count > 0
                    variant: "text"
                    size: "sm"
                    text: qsTr("Clear history")
                    onClicked: NotificationList.clearHistory()
                }
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "bodySmall"
                muted: true
                text: NotificationHistory.count > 0
                      ? qsTr("Dismissed notifications from the last 24 hours.")
                      : qsTr("No recent notifications.")
            }
            // Search: by app, title or text, ignoring case.
            Rectangle {
                width: parent.width
                height: 40
                visible: NotificationHistory.count > 0
                radius: Theme.graphite ? Theme.radiusSm : (Theme.quiet ? Theme.radiusMd : height / 2)
                color: Theme.graphite ? "transparent" : Theme.raised
                border.width: historySearch.activeFocus ? 2 : 1
                border.color: historySearch.activeFocus ? Theme.focusRing : Theme.outlineVariant
                Icon {
                    id: searchIcon
                    anchors.left: parent.left
                    anchors.leftMargin: 14
                    anchors.verticalCenter: parent.verticalCenter
                    width: 18; height: 18
                    path: Icons.search
                    color: Theme.surfaceContentVariant
                }
                TextInput {
                    id: historySearch
                    readonly property string query: text.trim().toLocaleLowerCase()
                    function matches(fields) {
                        return query.length === 0 || fields.join(" ").toLocaleLowerCase().indexOf(query) >= 0
                    }
                    anchors.left: searchIcon.right
                    anchors.leftMargin: 10
                    anchors.right: parent.right
                    anchors.rightMargin: 16
                    anchors.verticalCenter: parent.verticalCenter
                    font.family: Theme.fontUi
                    font.pixelSize: 14
                    color: Theme.surfaceContent
                    selectionColor: Theme.primaryContainer
                    selectedTextColor: Theme.primaryContainerContent
                    clip: true
                    Keys.onEscapePressed: (event) => {
                        // Clears first; a second Escape closes the sheet.
                        event.accepted = text.length > 0
                        text = ""
                    }
                    Accessible.name: qsTr("Search notification history")
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: historySearch.text.length === 0
                        role: "body"
                        muted: true
                        text: qsTr("Search notification history")
                    }
                }
            }
            Flickable {
                width: parent.width
                height: Math.min(historyColumn.height, page.height - 260)
                contentHeight: historyColumn.height
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                Column {
                    id: historyColumn
                    width: parent.width
                    Repeater {
                        model: historySheet.opened ? NotificationHistory : null
                        delegate: HistoryItem {
                            required property string deviceId
                            required property string app
                            width: historyColumn.width
                            visible: historySearch.matches([appName, title, text, sub])
                            canOpenApp: (AppController.capsRevision >= 0
                                && AppController.featureState(deviceId, "mirroring.app_windows").state === "available")
                                && app.length > 0
                            onOpenAppRequested: {
                                historySheet.close()
                                Mirror.startApp(deviceId, app, appName.length > 0 ? appName : app)
                            }
                        }
                    }
                }
            }
            Txt {
                width: parent.width
                visible: historySearch.query.length > 0 && historyColumn.height === 0
                horizontalAlignment: Text.AlignHCenter
                role: "body"
                muted: true
                text: qsTr("Nothing matches “%1”.").arg(historySearch.text.trim())
            }
            Row {
                anchors.right: parent.right
                Button { text: qsTr("Done"); onClicked: historySheet.close() }
            }
        }
    }

    Sheet {
        id: clipboardSheet
        cardWidth: 560
        readonly property var items: {
            if (!clipboardSheet.opened)
                return []
            try {
                return JSON.parse(AppController.clipboardHistory)
            } catch (e) {
                return []
            }
        }
        Column {
            width: parent.width
            spacing: 12
            Item {
                width: parent.width
                height: Math.max(clipboardTitle.height, clearClipboard.height)
                Txt { id: clipboardTitle; anchors.verticalCenter: parent.verticalCenter; text: qsTr("Clipboard history"); role: "headline" }
                Button {
                    id: clearClipboard
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    visible: clipboardSheet.items.length > 0
                    variant: "text"
                    size: "sm"
                    text: qsTr("Clear history")
                    onClicked: AppController.clearClipboardHistory()
                }
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "bodySmall"
                muted: true
                text: clipboardSheet.items.length > 0
                      ? qsTr("Last 50 shared clips.")
                      : qsTr("No shared clips yet.")
            }
            Rectangle {
                width: parent.width
                height: 40
                visible: clipboardSheet.items.length > 0
                radius: Theme.graphite ? Theme.radiusSm : (Theme.quiet ? Theme.radiusMd : height / 2)
                color: Theme.graphite ? "transparent" : Theme.raised
                border.width: clipboardSearch.activeFocus ? 2 : 1
                border.color: clipboardSearch.activeFocus ? Theme.focusRing : Theme.outlineVariant
                Icon {
                    id: clipSearchIcon
                    anchors.left: parent.left
                    anchors.leftMargin: 14
                    anchors.verticalCenter: parent.verticalCenter
                    width: 18; height: 18
                    path: Icons.search
                    color: Theme.surfaceContentVariant
                }
                TextInput {
                    id: clipboardSearch
                    readonly property string query: text.trim().toLocaleLowerCase()
                    function matches(fields) {
                        return query.length === 0 || fields.join(" ").toLocaleLowerCase().indexOf(query) >= 0
                    }
                    anchors.left: clipSearchIcon.right
                    anchors.leftMargin: 10
                    anchors.right: parent.right
                    anchors.rightMargin: 16
                    anchors.verticalCenter: parent.verticalCenter
                    font.family: Theme.fontUi
                    font.pixelSize: 14
                    color: Theme.surfaceContent
                    selectionColor: Theme.primaryContainer
                    selectedTextColor: Theme.primaryContainerContent
                    clip: true
                    Keys.onEscapePressed: (event) => {
                        event.accepted = text.length > 0
                        text = ""
                    }
                    Accessible.name: qsTr("Search clipboard history")
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: clipboardSearch.text.length === 0
                        role: "body"
                        muted: true
                        text: qsTr("Search clipboard history")
                    }
                }
            }
            Flickable {
                width: parent.width
                height: Math.min(clipboardColumn.height, page.height - 260)
                contentHeight: clipboardColumn.height
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                Column {
                    id: clipboardColumn
                    width: parent.width
                    Repeater {
                        model: clipboardSheet.items
                        delegate: Item {
                            id: clipRow
                            required property var modelData
                            required property int index
                            readonly property string sourceLabel: modelData.incoming
                                ? (modelData.deviceName ? qsTr("From %1").arg(modelData.deviceName) : qsTr("From phone"))
                                : (modelData.deviceName ? qsTr("Sent to %1").arg(modelData.deviceName) : qsTr("From this PC"))
                            readonly property string timeLabel: page.relativeTime(modelData.timestamp || 0)
                            width: clipboardColumn.width
                            visible: clipboardSearch.matches([
                                modelData.text || "",
                                sourceLabel,
                                modelData.kind === "image" ? qsTr("Image") : ""
                            ])
                            height: visible ? Math.max(clipIconBox.height, clipContent.height, clipActions.height) + 20 : 0

                            Divider {
                                width: parent.width
                                visible: clipRow.index > 0
                            }

                            Item {
                                id: clipIconBox
                                x: 4; y: 12
                                width: 28; height: 28
                                Icon {
                                    anchors.centerIn: parent
                                    width: 18; height: 18
                                    path: clipRow.modelData.pinned
                                          ? Icons.pin
                                          : (clipRow.modelData.incoming ? Icons.phone : Icons.laptop)
                                    color: clipRow.modelData.pinned ? Theme.accent : Theme.surfaceContentVariant
                                }
                            }

                            Column {
                                id: clipContent
                                anchors.left: clipIconBox.right
                                anchors.leftMargin: 12
                                anchors.right: clipActions.left
                                anchors.rightMargin: 8
                                y: 10
                                spacing: 4
                                Txt {
                                    width: parent.width
                                    role: "caption"
                                    muted: true
                                    text: [
                                        clipRow.sourceLabel,
                                        clipRow.timeLabel,
                                        clipRow.modelData.pinned ? qsTr("Pinned") : ""
                                    ].filter(s => s.length > 0).join(" · ")
                                    elide: Text.ElideRight
                                }
                                Txt {
                                    width: parent.width
                                    visible: clipRow.modelData.kind === "text" && Boolean(clipRow.modelData.text)
                                    role: "bodySmall"
                                    text: clipRow.modelData.text || ""
                                    wrapMode: Text.Wrap
                                    maximumLineCount: 3
                                    elide: Text.ElideRight
                                }
                                Rectangle {
                                    visible: clipRow.modelData.kind === "image"
                                    width: Math.min(parent.width, 180)
                                    height: 88
                                    radius: Theme.radiusSm
                                    color: Theme.surfaceContainerLow
                                    border.width: 1
                                    border.color: Theme.outlineVariant
                                    clip: true
                                    Image {
                                        anchors.fill: parent
                                        anchors.margins: 4
                                        source: clipRow.modelData.imageDataUrl || ""
                                        fillMode: Image.PreserveAspectFit
                                        asynchronous: true
                                        mipmap: true
                                    }
                                }
                            }

                            Row {
                                id: clipActions
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 4
                                IconButton {
                                    visible: page.currentDeviceId.length > 0
                                             && clipRow.modelData.kind === "text"
                                             && Boolean(clipRow.modelData.text)
                                             && AppController.isHandoffText(clipRow.modelData.text)
                                    iconPath: Icons.globe
                                    label: qsTr("Open on phone")
                                    onClicked: {
                                        clipboardSheet.close()
                                        AppController.openLinkOnPhone(page.currentDeviceId, clipRow.modelData.text)
                                    }
                                }
                                IconButton {
                                    visible: page.currentDeviceId.length > 0
                                    iconPath: Icons.send
                                    label: qsTr("Send to phone")
                                    onClicked: {
                                        clipboardSheet.close()
                                        AppController.sendClipboardHistory(page.currentDeviceId, clipRow.modelData.id)
                                    }
                                }
                                IconButton {
                                    iconPath: Icons.copy
                                    label: qsTr("Copy")
                                    onClicked: AppController.copyClipboardHistory(clipRow.modelData.id)
                                }
                                IconButton {
                                    iconPath: Icons.pin
                                    iconColor: clipRow.modelData.pinned ? Theme.accent : Theme.surfaceContentVariant
                                    label: clipRow.modelData.pinned ? qsTr("Unpin") : qsTr("Pin")
                                    onClicked: AppController.pinClipboardHistory(clipRow.modelData.id, !clipRow.modelData.pinned)
                                }
                                IconButton {
                                    iconPath: Icons.trash
                                    label: qsTr("Delete")
                                    onClicked: AppController.deleteClipboardHistory(clipRow.modelData.id)
                                }
                            }
                        }
                    }
                }
            }
            Txt {
                width: parent.width
                visible: clipboardSearch.query.length > 0 && clipboardColumn.height === 0
                horizontalAlignment: Text.AlignHCenter
                role: "body"
                muted: true
                text: qsTr("Nothing matches “%1”.").arg(clipboardSearch.text.trim())
            }
            Row {
                anchors.right: parent.right
                Button { text: qsTr("Done"); onClicked: clipboardSheet.close() }
            }
        }
    }

    // An action on the phone, as a row. Unavailable, it's muted and says what
    // unlocks it (or "When connected" until the phone has said).
    component PhoneAction: ActionRow {
        id: action
        property var feature: ({})
        property color glyphTint: Theme.surfaceContentVariant
        property bool queueable: false
        property bool addonOk: false
        property bool forceAvailable: false
        property bool active: false
        property bool deviceOnline: true
        readonly property bool available: forceAvailable
            || page.featureUsable(feature, deviceOnline, queueable, addonOk)
        width: parent ? parent.width : 0
        interactive: available
        tint: available ? glyphTint : Theme.surfaceContentVariant
        titleColor: available ? Theme.surfaceContent : Theme.surfaceContentVariant
        selected: active
        meta: available ? "" : (lock.visible ? "" : qsTr("When connected"))
        LockChip {
            id: lock
            anchors.verticalCenter: parent.verticalCenter
            visible: !action.available && label.length > 0 && (action.deviceOnline || action.feature.action !== "updateApp")
            feature: action.feature
            maxWidth: 200
        }
    }

    // A phone setting you flip: glyph and name; on, it takes the accent.
    component ToggleTile: FocusScope {
        id: tile
        property string iconPath
        property string label
        // The whole name, for the tooltip and screen readers, when `label`
        // is shortened to fit the tile.
        property string fullName: label
        property bool active: false
        property var feature: ({})
        readonly property bool available: feature.state === "available"
        signal clicked
        signal lockedClicked

        height: 34
        activeFocusOnTab: true
        Accessible.role: Accessible.CheckBox
        Accessible.name: fullName
        Accessible.checked: active
        Accessible.onPressAction: tile.available ? tile.clicked() : tile.lockedClicked()
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                if (tile.available)
                    tile.clicked()
                else
                    tile.lockedClicked()
                event.accepted = true
            }
        }
        readonly property color ink: tile.active && tile.available ? Theme.accentContent
            : (tile.available ? Theme.surfaceContent : Theme.surfaceContentVariant)

        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: tile.active && tile.available ? Theme.accent
                : (tileHover.hovered && tile.available ? Theme.selection : Theme.raised)
            opacity: tile.available || tile.active ? 1 : 0.6
            border.width: Theme.focusVisible(tile) ? 2 : 0
            border.color: Theme.focusRing
        }
        Icon {
            id: tileGlyph
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            width: 16; height: 16
            path: tile.available ? tile.iconPath : Icons.lock
            color: tile.ink
        }
        Txt {
            anchors.left: tileGlyph.right
            anchors.leftMargin: 8
            anchors.right: parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            role: "bodySmall"
            color: tile.ink
            text: tile.label
        }
        HoverHandler { id: tileHover; cursorShape: Qt.PointingHandCursor }
        TapHandler {
            gesturePolicy: TapHandler.ReleaseWithinBounds
            onTapped: tile.available ? tile.clicked() : tile.lockedClicked()
        }
        // The full name above the tile, after a short hover.
        Timer {
            id: tipDelay
            interval: 450
            running: tileHover.hovered && tile.fullName !== tile.label
        }
        Rectangle {
            z: 10
            visible: tileHover.hovered && tile.fullName !== tile.label && !tipDelay.running
            x: Math.round((tile.width - width) / 2)
            y: -height - 6
            width: tipText.implicitWidth + 18
            height: tipText.implicitHeight + 10
            radius: Theme.radiusSm
            color: Theme.surfaceContainerHighest
            border.width: 1
            border.color: Theme.line
            Txt {
                id: tipText
                anchors.centerIn: parent
                role: "caption"
                text: tile.fullName
            }
        }
    }

    // Debounced 0..=100 slider for a phone setting (media volume, screen brightness).
    component ToggleSlider: Column {
        id: slider
        property string deviceId
        property string toggleId
        property string title
        property string iconPath
        property int phoneValue: 0
        property var feature: ({})
        readonly property bool available: feature.state === "available"
        property int dragValue: -1
        readonly property int shownValue: dragValue >= 0 ? dragValue : Math.max(0, Math.min(100, phoneValue))

        spacing: 4

        Timer {
            id: sendDebounce
            interval: 100
            onTriggered: if (slider.dragValue >= 0) {
                AppController.setPhoneToggle(slider.deviceId, slider.toggleId, String(slider.dragValue))
            }
        }
        Timer {
            id: settle
            interval: 400
            onTriggered: if (!dragArea.pressed) slider.dragValue = -1
        }

        Item {
            width: parent.width
            height: 20
            Row {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 15; height: 15
                    path: slider.iconPath
                    color: slider.available ? Theme.surfaceContent : Theme.surfaceContentVariant
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    role: "bodySmall"
                    muted: !slider.available
                    text: slider.title
                }
            }
            Txt {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                role: "caption"
                muted: true
                text: qsTr("%1%").arg(slider.shownValue)
            }
        }

        FocusScope {
            id: barScope
            width: parent.width
            height: 18
            activeFocusOnTab: slider.available
            Accessible.role: Accessible.Slider
            Accessible.name: slider.title
            Accessible.description: qsTr("%1%").arg(slider.shownValue)
            Accessible.onPressAction: if (slider.available) {
                const next = slider.shownValue >= 80 ? 25 : slider.shownValue + 20
                slider.dragValue = next
                settle.restart()
                AppController.setPhoneToggle(slider.deviceId, slider.toggleId, String(next))
            }
            Keys.onPressed: (event) => {
                if (!slider.available) return
                if (event.key === Qt.Key_Left || event.key === Qt.Key_Down) {
                    const next = Math.max(0, slider.shownValue - 5)
                    slider.dragValue = next
                    settle.restart()
                    AppController.setPhoneToggle(slider.deviceId, slider.toggleId, String(next))
                    event.accepted = true
                } else if (event.key === Qt.Key_Right || event.key === Qt.Key_Up) {
                    const next = Math.min(100, slider.shownValue + 5)
                    slider.dragValue = next
                    settle.restart()
                    AppController.setPhoneToggle(slider.deviceId, slider.toggleId, String(next))
                    event.accepted = true
                }
            }

            readonly property real fraction: slider.shownValue / 100.0

            Rectangle {
                anchors.fill: parent
                anchors.margins: -3
                radius: 6
                color: "transparent"
                border.width: 2
                border.color: Theme.focusRing
                visible: Theme.focusVisible(barScope)
            }

            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 4
                radius: 2
                color: Theme.selection
                border.width: Theme.graphite ? 1 : 0
                border.color: Theme.outlineVariant
                opacity: slider.available ? 1 : 0.45

                Rectangle {
                    id: sliderFill
                    width: parent.width * barScope.fraction
                    height: parent.height
                    radius: parent.radius
                    color: slider.available
                           ? Theme.accent
                           : Theme.surfaceContentVariant
                    Behavior on width {
                        enabled: !Theme.reduceMotion && !dragArea.pressed
                        NumberAnimation { duration: 120; easing.type: Easing.OutCubic }
                    }
                }
            }

            Rectangle {
                visible: slider.available
                x: Math.max(0, Math.min(barScope.width - width, sliderFill.width - width / 2))
                anchors.verticalCenter: parent.verticalCenter
                width: 14; height: 14; radius: 7
                color: Theme.accent
                border.width: Theme.focusVisible(barScope) ? 2 : 0
                border.color: Theme.surface
                scale: dragArea.pressed ? 1.15 : 1.0
                Behavior on scale {
                    enabled: !Theme.reduceMotion
                    NumberAnimation { duration: 150; easing.type: Easing.OutCubic }
                }
            }

            MouseArea {
                id: dragArea
                anchors.fill: parent
                anchors.margins: -4
                enabled: slider.available
                hoverEnabled: true
                cursorShape: slider.available ? Qt.PointingHandCursor : Qt.ArrowCursor
                function levelAt(x) {
                    return Math.round(Math.max(0, Math.min(1, (x - 4) / barScope.width)) * 100)
                }
                onPressed: (mouse) => {
                    settle.stop()
                    slider.dragValue = levelAt(mouse.x)
                    if (!sendDebounce.running) sendDebounce.start()
                }
                onPositionChanged: (mouse) => {
                    if (pressed) {
                        slider.dragValue = levelAt(mouse.x)
                        if (!sendDebounce.running) sendDebounce.start()
                    }
                }
                onReleased: (mouse) => {
                    sendDebounce.stop()
                    const finalVal = levelAt(mouse.x)
                    slider.dragValue = finalVal
                    AppController.setPhoneToggle(slider.deviceId, slider.toggleId, String(finalVal))
                    settle.restart()
                }
                onCanceled: {
                    sendDebounce.stop()
                    slider.dragValue = -1
                }
            }
        }

        LockChip {
            visible: !slider.available && label.length > 0 && slider.feature.action !== "enableToggle"
            feature: slider.feature
            maxWidth: parent.width
        }
    }

    // Nearby devices running LocalSend on the local Wi-Fi network.
    component LocalSendSection: Column {
        spacing: 2
        Item {
            width: parent.width
            height: 30
            Row {
                x: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    role: "label"
                    muted: true
                    text: qsTr("LocalSend nearby")
                }
                StatusDot {
                    anchors.verticalCenter: parent.verticalCenter
                    online: AppController.localsendReceiving
                }
            }
            TextAction {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Refresh")
                onClicked: AppController.refreshLocalsend()
            }
        }
        Txt {
            x: Theme.rowPadding
            width: parent.width - 2 * Theme.rowPadding
            visible: page.localsendPeers.length === 0
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            text: AppController.localsendReceiving
                ? qsTr("Listening on Wi‑Fi")
                : qsTr("Port 53317 is busy (another LocalSend app?)")
        }
        Repeater {
            model: page.localsendPeers
            delegate: ActionRow {
                id: peerRow
                required property var modelData
                width: parent ? parent.width : 0
                iconPath: modelData.deviceType === "mobile" || modelData.deviceType === "tablet"
                    ? Icons.phone : Icons.laptop
                tint: Theme.tintBlue
                title: modelData.alias || qsTr("Nearby device")
                detail: [modelData.deviceModel, modelData.ip].filter(s => s && s.length > 0).join(" · ")
                onClicked: {
                    localsendFilePicker.targetDeviceId = peerRow.modelData.id
                    localsendFilePicker.open()
                }
                TextAction {
                    text: qsTr("Send files")
                    onClicked: {
                        localsendFilePicker.targetDeviceId = peerRow.modelData.id
                        localsendFilePicker.open()
                    }
                }
            }
        }
    }


    // Inline lightbox preview for notification images.
    Sheet {
        id: imagePreviewSheet
        property string imageUrl: ""
        property string caption: ""
        cardWidth: 680
        function openFor(url, title) {
            imageUrl = url || ""
            caption = title || ""
            if (imageUrl.length > 0)
                open()
        }
        Column {
            width: parent.width
            spacing: 14
            Item {
                width: parent.width
                height: Math.max(previewHeaderTxt.height, closePreviewBtn.height)
                Txt {
                    id: previewHeaderTxt
                    anchors.left: parent.left
                    anchors.right: closePreviewBtn.left
                    anchors.rightMargin: 12
                    anchors.verticalCenter: parent.verticalCenter
                    role: "headline"
                    elide: Text.ElideRight
                    text: imagePreviewSheet.caption.length > 0 ? imagePreviewSheet.caption : qsTr("Image preview")
                }
                IconButton {
                    id: closePreviewBtn
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: Icons.close
                    label: qsTr("Close")
                    onClicked: imagePreviewSheet.close()
                }
            }
            Rectangle {
                width: parent.width
                height: Math.min(480, Math.max(200, previewImg.status === Image.Ready
                    ? width * previewImg.implicitHeight / Math.max(1, previewImg.implicitWidth)
                    : 320))
                radius: Theme.radiusMd
                color: Theme.surfaceContainerHighest
                clip: true
                Image {
                    id: previewImg
                    anchors.fill: parent
                    anchors.margins: 8
                    source: imagePreviewSheet.imageUrl
                    fillMode: Image.PreserveAspectFit
                    smooth: true
                    mipmap: true
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 8
                Button {
                    variant: "tonal"
                    iconPath: Icons.photo
                    text: qsTr("Open externally")
                    onClicked: Qt.openUrlExternally(imagePreviewSheet.imageUrl)
                }
                Button {
                    text: qsTr("Done")
                    onClicked: imagePreviewSheet.close()
                }
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 6
        radius: Theme.radiusXl
        color: "transparent"
        border.width: 2
        border.color: Theme.accent
        visible: opacity > 0
        opacity: AppController.highlightTarget.startsWith("home:") ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }
}
