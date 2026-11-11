// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Layouts
import app.g7link

// The Guide: every feature, grouped, with its live state on the phone on
// screen (left), and the one you pick explained (right): where it is, what
// it needs, Show me and the handbook chapter.
Item {
    id: guide

    property bool active: true
    property string deviceId: ""
    property string deviceName: ""
    property bool deviceOnline: false
    readonly property bool hasConnectedPhone: guide.deviceId !== "" && guide.deviceOnline

    signal showMeRequested(string targetId)
    signal restartWalkthroughRequested()

    property string searchQuery: ""
    property string selectedArea: "all"
    property string activeDocKey: "overview"
    property string selectedId: ""

    property var deviceTogglesMap: ({
        notifications: true,
        clipboard: true,
        files: true,
        sms: true,
        calls: true,
        media: true,
        storage: false,
        remote_input: false,
        pc_mirror: false
    })

    function openDoc(key) {
        activeDocKey = key
        AppController.openGuideDoc(key)
    }

    function syncSelectedDevice() {
        let _caps = AppController.capsRevision
        let _toggles = AppController.togglesRevision
        let count = DeviceList.count
        if (count <= 0) {
            deviceOnline = false
            return
        }
        // A phone that was unpaired since isn't the one to show.
        let stillThere = false
        for (let i = 0; i < count; i++) {
            if (DeviceList.deviceIdAt(i) === guide.deviceId)
                stillThere = true
        }
        if (!stillThere)
            guide.deviceId = ""
        let targetId = guide.deviceId.length > 0 ? guide.deviceId : (DeviceList.deviceIdAt(0) || "")
        if (guide.deviceId.length === 0 && targetId.length > 0) {
            guide.deviceId = targetId
            guide.deviceName = DeviceList.deviceNameAt(0) || qsTr("Phone")
        }
        if (targetId !== "") {
            let st = AppController.featureState(targetId, "files")
            let reason = st ? (st.reason || "") : ""
            deviceOnline = reason.toLowerCase().indexOf("offline") === -1
            let list = AppController.deviceToggles(targetId)
            let next = {
                notifications: true,
                clipboard: true,
                files: true,
                sms: true,
                calls: true,
                media: true,
                storage: false,
                remote_input: false,
                pc_mirror: false
            }
            for (let i = 0; i < list.length; i++) {
                let entry = list[i]
                if (entry && entry.name !== undefined)
                    next[entry.name] = Boolean(entry.on)
            }
            deviceTogglesMap = next
        }
    }

    onDeviceIdChanged: syncSelectedDevice()
    Component.onCompleted: syncSelectedDevice()

    Connections {
        target: AppController
        function onHasDevicesChanged() { guide.syncSelectedDevice() }
        function onCapsRevisionChanged() { guide.syncSelectedDevice() }
        function onTogglesRevisionChanged() { guide.syncSelectedDevice() }
    }


    readonly property var guideChapters: [
        { key: "overview", label: qsTr("Overview") },
        { key: "setup", label: qsTr("Setup") },
        { key: "permissions", label: qsTr("Permissions") },
        { key: "elevated", label: qsTr("Power levels") },
        { key: "troubleshooting", label: qsTr("Troubleshooting") },
        { key: "faq", label: qsTr("FAQ") }
    ]

    readonly property var allFeatures: [
        {
            id: "notifications",
            name: qsTr("Notifications"),
            icon: Icons.bell,
            area: "notifications_calls",
            capKey: "notifications",
            toggleKey: "notifications",
            summary: qsTr("Mirror phone notifications to your PC with inline replies, action buttons, and dismissal sync."),
            whereLabel: qsTr("Home › Notifications"),
            needsLabel: qsTr("Notification Access on phone"),
            targetId: "home:notifications",
            docKey: "notifications"
        },
        {
            id: "messages",
            name: qsTr("Messages"),
            icon: Icons.messages,
            area: "messages",
            capKey: "sms",
            toggleKey: "sms",
            summary: qsTr("Read and send SMS and MMS threads, or reply to notification-backed chat conversations."),
            whereLabel: qsTr("Navigation › Messages"),
            needsLabel: qsTr("SMS and Contacts permissions on phone"),
            targetId: "messages:main",
            docKey: "messages"
        },
        {
            id: "calls",
            name: qsTr("Calls"),
            icon: Icons.call,
            area: "notifications_calls",
            capKey: "calls",
            toggleKey: "calls",
            summary: qsTr("Answer or decline incoming calls, browse call history and contacts, and dial from your PC."),
            whereLabel: qsTr("Navigation › Calls"),
            needsLabel: qsTr("Phone, Call Log, and Contacts permissions on phone"),
            targetId: "calls:main",
            docKey: "calls"
        },
        {
            id: "now_playing",
            name: qsTr("Now playing"),
            icon: Icons.music,
            area: "notifications_calls",
            capKey: "media_view",
            toggleKey: "media",
            summary: qsTr("See active track artwork and control playback between your phone and PC."),
            whereLabel: qsTr("Home › Media player"),
            needsLabel: qsTr("Notification Access on phone"),
            targetId: "deck:main",
            docKey: "deck"
        },
        {
            id: "clipboard",
            name: qsTr("Clipboard"),
            icon: Icons.clipboard,
            area: "clipboard_files",
            capKey: "clipboard_send",
            toggleKey: "clipboard",
            summary: qsTr("Sync copied text, HTML, images, and files with a 50-item local clipboard history."),
            whereLabel: qsTr("Home › Send clipboard"),
            needsLabel: qsTr("Clipboard toggle on Home; Elevated or quick tile for background phone copy"),
            targetId: "home:clipboard",
            docKey: "clipboard"
        },
        {
            id: "transfers",
            name: qsTr("Send files"),
            icon: Icons.send,
            area: "clipboard_files",
            capKey: "files",
            toggleKey: "files",
            summary: qsTr("Send files, folders, and links to your phone over your local Wi-Fi network."),
            whereLabel: qsTr("Home › Send files"),
            needsLabel: qsTr("Send files toggle on Home"),
            targetId: "home:transfers",
            docKey: "transfers"
        },
        {
            id: "send_queue",
            name: qsTr("Send queue"),
            icon: Icons.send,
            area: "clipboard_files",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            localAlways: true,
            summary: qsTr("Queue files and links while your phone is away so they send automatically on reconnect."),
            whereLabel: qsTr("Home › Send queue"),
            needsLabel: qsTr("Always available on PC"),
            targetId: "home:transfers",
            docKey: "transfers"
        },
        {
            id: "photos",
            name: qsTr("Photos"),
            icon: Icons.photo,
            area: "clipboard_files",
            capKey: "photos",
            toggleKey: "files",
            summary: qsTr("Browse recent phone photos and screenshots on demand, copy to clipboard, or drag into apps."),
            whereLabel: qsTr("Navigation › Photos"),
            needsLabel: qsTr("Photos and Media permission on phone"),
            targetId: "photos:main",
            docKey: "photos"
        },
        {
            id: "mirroring",
            name: qsTr("Screen mirroring"),
            icon: Icons.mirror,
            area: "mirroring_apps",
            capKey: "mirror_view",
            toggleKey: "pc_mirror",
            summary: qsTr("Stream your phone display in a low-latency desktop window with mouse and keyboard input."),
            whereLabel: qsTr("Home › Mirror screen"),
            needsLabel: qsTr("MediaProjection approval on phone; Assist or Elevated for touch input"),
            targetId: "home:mirror",
            docKey: "mirroring"
        },
        {
            id: "app_streams",
            name: qsTr("App streams"),
            icon: Icons.apps,
            area: "mirroring_apps",
            capKey: "mirror_view",
            toggleKey: "",
            summary: qsTr("Open individual Android apps in their own virtual display windows on your PC."),
            whereLabel: qsTr("Apps"),
            needsLabel: qsTr("Elevated (Wireless debugging) enabled on phone"),
            targetId: "apps:",
            docKey: "mirroring"
        },
        {
            id: "pc_mirror",
            name: qsTr("Reverse PC screen mirroring"),
            icon: Icons.desktop,
            area: "mirroring_apps",
            capKey: "pc_mirror",
            toggleKey: "pc_mirror",
            summary: qsTr("Stream this PC monitor to your phone and control the cursor or laser pointer from your phone."),
            whereLabel: qsTr("Phone app › Home, and Settings › Devices"),
            needsLabel: qsTr("Opt-in Screen mirroring toggle on Home"),
            targetId: "home:pc_mirror",
            docKey: "pc-mirror"
        },
        {
            id: "webcam",
            name: qsTr("Phone as webcam"),
            icon: Icons.video,
            area: "camera_audio",
            capKey: "webcam",
            toggleKey: "",
            summary: qsTr("Use your phone camera as the G7 Link Virtual Camera in Zoom, Teams, Meet, and OBS."),
            whereLabel: qsTr("Home › Phone as webcam"),
            needsLabel: qsTr("Camera permission on phone and Virtual Camera registered on PC"),
            targetId: "settings:camera_audio:webcam",
            docKey: "webcam"
        },
        {
            id: "continuity_camera",
            name: qsTr("Continuity camera"),
            icon: Icons.camera,
            area: "camera_audio",
            capKey: "webcam",
            toggleKey: "",
            summary: qsTr("Capture a photo or scan a document with your phone and paste it into the active PC window."),
            whereLabel: qsTr("Home › Continuity Camera (Ctrl+Alt+C / Ctrl+Alt+D)"),
            needsLabel: qsTr("Camera permission on phone"),
            targetId: "home:continuity",
            docKey: "continuity-camera"
        },
        {
            id: "speaker",
            name: qsTr("Phone as speaker"),
            icon: Icons.speaker,
            area: "camera_audio",
            capKey: "",
            toggleKey: "",
            prefKey: "streamPcAudioToPhone",
            summary: qsTr("Stream PC system audio to your phone in real time."),
            whereLabel: qsTr("Home › Phone as speaker"),
            needsLabel: qsTr("Phone as speaker enabled in Settings"),
            targetId: "settings:camera_audio:speaker",
            docKey: "speaker"
        },
        {
            id: "recordings",
            name: qsTr("Voice recorder"),
            icon: Icons.mic,
            area: "camera_audio",
            capKey: "files",
            toggleKey: "files",
            summary: qsTr("Record voice notes on your phone and save them automatically to your PC in M4A, MP3, WAV, or FLAC."),
            whereLabel: qsTr("Settings › Files & Sync › Voice recordings"),
            needsLabel: qsTr("Microphone permission on phone"),
            targetId: "settings:files_sync:recordings",
            docKey: "recordings"
        },
        {
            id: "deck",
            name: qsTr("Deck"),
            icon: Icons.deck,
            area: "camera_audio",
            capKey: "media_view",
            toggleKey: "media",
            summary: qsTr("Control Now playing media, switch PC audio outputs, and customize shortcut tiles for your phone."),
            whereLabel: qsTr("Navigation › Deck"),
            needsLabel: qsTr("Paired phone"),
            targetId: "deck:main",
            docKey: "deck"
        },
        {
            id: "desk_mode",
            name: qsTr("Desk mode"),
            icon: Icons.history,
            area: "surfaces",
            capKey: "media_view",
            toggleKey: "media",
            summary: qsTr("Turn your phone or PC window into a glanceable desk display with clock, media, and live status."),
            whereLabel: qsTr("Navigation › Deck › Desk mode"),
            needsLabel: qsTr("Paired phone"),
            targetId: "deck:main",
            docKey: "deck"
        },
        {
            id: "remote_input",
            name: qsTr("Remote input"),
            icon: Icons.mouse,
            area: "input_control",
            capKey: "remote_input",
            toggleKey: "remote_input",
            summary: qsTr("Use your phone as a wireless trackpad, keyboard, media remote, and presentation pointer."),
            whereLabel: qsTr("Phone app › Home › Control PC"),
            needsLabel: qsTr("Opt-in Remote input toggle on Home"),
            targetId: "home:remote_input",
            docKey: "remote-input"
        },
        {
            id: "type_on_phone",
            name: qsTr("Type on phone"),
            icon: Icons.keyboard,
            area: "input_control",
            capKey: "mirror_control",
            toggleKey: "",
            summary: qsTr("Send live keystrokes and pasted text from your PC keyboard to your phone without screen mirroring."),
            whereLabel: qsTr("Home › Type on phone"),
            needsLabel: qsTr("Assist (Accessibility) or Elevated (Wireless debugging) on phone"),
            targetId: "home:type_on_phone",
            docKey: "type-on-phone"
        },
        {
            id: "quick_actions",
            name: qsTr("Phone controls"),
            icon: Icons.toggles,
            area: "input_control",
            capKey: "find_device",
            toggleKey: "",
            summary: qsTr("Toggle Do Not Disturb, ringer mode, flashlight, volume, brightness, Wi-Fi, Bluetooth, and Find my phone."),
            whereLabel: qsTr("Home › Phone controls"),
            needsLabel: qsTr("Elevated (Wireless debugging) required only for Wi-Fi and Bluetooth toggles"),
            targetId: "home:phone_controls",
            docKey: "phone-controls"
        },
        {
            id: "shelf",
            name: qsTr("Drop shelf"),
            icon: Icons.shelf,
            area: "surfaces",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            localAlways: true,
            summary: qsTr("Slide-out top screen-edge shelf with your latest phone photo, clipboard items, and file drop zone."),
            whereLabel: qsTr("Top screen edge or Ctrl+Alt+S"),
            needsLabel: qsTr("Always available on PC"),
            targetId: "settings:shortcuts:shelf",
            docKey: "shelf"
        },
        {
            id: "g7_island",
            name: qsTr("G7 Island"),
            icon: Icons.island,
            area: "surfaces",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            prefKey: "g7Island",
            summary: qsTr("Compact floating pill at the top center of your monitor for calls, transfers, media, and alerts."),
            whereLabel: qsTr("Settings › General › G7 Island"),
            needsLabel: qsTr("G7 Island enabled in Settings"),
            targetId: "settings:general:island",
            docKey: "g7-island"
        },
        {
            id: "timeline",
            name: qsTr("Timeline"),
            icon: Icons.history,
            area: "surfaces",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            localAlways: true,
            summary: qsTr("Chronological history of shared files, photos, clipboard items, calls, and voice recordings."),
            whereLabel: qsTr("Top bar › Timeline"),
            needsLabel: qsTr("Always available on PC"),
            targetId: "timeline:main",
            docKey: "timeline"
        },
        {
            id: "command_palette",
            name: qsTr("Command palette"),
            icon: Icons.search,
            area: "surfaces",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            localAlways: true,
            summary: qsTr("Keyboard-first search over actions, contacts, message threads, mirrorable apps, and quick toggles."),
            whereLabel: qsTr("Top bar › Commands, or Ctrl+K (Ctrl+Alt+Space anywhere)"),
            needsLabel: qsTr("Always available on PC"),
            targetId: "settings:shortcuts:palette",
            docKey: "command-palette"
        },
        {
            id: "folder_sync",
            name: qsTr("Share PC folders"),
            icon: Icons.folder,
            area: "clipboard_files",
            capKey: "storage",
            toggleKey: "storage",
            pcLocal: true,
            summary: qsTr("Share selected PC folders with your phone or sync folders in one-way or two-way mode."),
            whereLabel: qsTr("Settings › Files & Sync › Synced folders"),
            needsLabel: qsTr("Opt-in Share PC folders toggle on Home"),
            targetId: "settings:files_sync:folder_sync",
            docKey: "folder-sync"
        },
        {
            id: "wol",
            name: qsTr("Wake PC"),
            icon: Icons.power,
            area: "automation_network",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            wolCheck: true,
            summary: qsTr("Let your paired phone wake this PC from sleep over your local network using Wake-on-LAN."),
            whereLabel: qsTr("Settings › Connection & About › Wake from your phone"),
            needsLabel: qsTr("Network adapter with Wake-on-LAN enabled on this PC"),
            targetId: "settings:connection:wol",
            docKey: "wol"
        },
        {
            id: "doctor",
            name: qsTr("Connection Doctor"),
            icon: Icons.doctor,
            area: "automation_network",
            capKey: "",
            toggleKey: "",
            pcLocal: true,
            localAlways: true,
            summary: qsTr("Diagnose and fix firewall rules and local discovery issues."),
            whereLabel: qsTr("Settings › Connection & About › Connection Doctor"),
            needsLabel: qsTr("Always available on PC"),
            targetId: "doctor:open",
            docKey: "doctor"
        }
    ]



    readonly property var areaChips: [
        { id: "all", label: qsTr("All") },
        { id: "notifications_calls", label: qsTr("Stay in touch") },
        { id: "messages", label: qsTr("Messages") },
        { id: "clipboard_files", label: qsTr("Move things") },
        { id: "mirroring_apps", label: qsTr("Use the phone") },
        { id: "camera_audio", label: qsTr("Camera and sound") },
        { id: "input_control", label: qsTr("Control the PC") },
        { id: "surfaces", label: qsTr("On the desktop") },
        { id: "automation_network", label: qsTr("Network") }
    ]

    // Each feature's glyph tint.
    function tintFor(item) {
        switch (item.id) {
        case "notifications": case "messages": case "timeline": return Theme.tintBlue
        case "calls": case "speaker": case "remote_input": case "doctor": return Theme.tintTeal
        case "clipboard": case "command_palette": case "g7_island": return Theme.tintLavender
        case "transfers": case "send_queue": case "folder_sync": case "shelf": return Theme.tintAmber
        case "photos": case "webcam": case "continuity_camera": return Theme.tintCoral
        case "mirroring": case "app_streams": case "now_playing": case "type_on_phone": return Theme.tintPink
        case "deck": case "desk_mode": case "quick_actions": return Theme.tintGreen
        case "wol": case "recordings": return Theme.tintRed
        default: return Theme.tintBlue
        }
    }
    function toneColor(tone) {
        return tone === "available" ? Theme.tintTeal
            : (tone === "basic" || tone === "warn") ? Theme.tintAmber : Theme.surfaceContentVariant
    }

    readonly property var filteredFeatures: {
        let q = searchQuery.trim().toLowerCase()
        let out = []
        for (let i = 0; i < allFeatures.length; i++) {
            let item = allFeatures[i]
            if (selectedArea !== "all" && item.area !== selectedArea)
                continue
            if (q !== "") {
                let hay = (item.name + " " + item.summary + " " + item.whereLabel + " " + item.needsLabel).toLowerCase()
                if (hay.indexOf(q) === -1)
                    continue
            }
            out.push(item)
        }
        return out
    }
    // The shown features by area, in the areas' order.
    readonly property var groups: {
        const list = []
        for (let a = 1; a < areaChips.length; a++) {
            const rows = filteredFeatures.filter((f) => f.area === areaChips[a].id)
            if (rows.length > 0)
                list.push({ label: areaChips[a].label, rows: rows })
        }
        return list
    }
    readonly property var orderedFeatures: groups.reduce((all, g) => all.concat(g.rows), [])
    readonly property var selectedFeature: {
        const list = orderedFeatures
        for (let i = 0; i < list.length; i++) {
            if (list[i].id === selectedId)
                return list[i]
        }
        return list.length > 0 ? list[0] : null
    }
    readonly property var selectedStatus: selectedFeature ? statusForFeature(selectedFeature) : null
    readonly property int readyCount: {
        let n = 0
        for (let i = 0; i < allFeatures.length; i++) {
            if (statusForFeature(allFeatures[i]).tone === "available")
                n++
        }
        return n
    }

    function step(delta) {
        const list = orderedFeatures
        if (list.length === 0)
            return
        let i = list.indexOf(selectedFeature)
        i = Math.max(0, Math.min(list.length - 1, i + delta))
        selectedId = list[i].id
    }
    function showSelected() {
        if (selectedFeature)
            guide.showMeRequested(selectedFeature.targetId)
    }

    // ---- Footer and Actions (MainWindow reads these) ----
    readonly property string footerLabel: qsTr("Guide · %1 of %2 ready").arg(readyCount).arg(allFeatures.length)
    readonly property var primaryAction: selectedFeature
        ? { label: qsTr("Show me"), keys: "Enter", run: () => guide.showSelected() } : null
    readonly property var secondaryAction: selectedFeature
        ? { label: qsTr("Learn more"), run: () => guide.openDoc(guide.selectedFeature.docKey) } : null
    readonly property var screenActions: {
        const list = []
        if (selectedFeature) {
            const f = selectedFeature
            list.push({ label: qsTr("Show me"), detail: f.name, icon: f.icon, tint: tintFor(f), keys: "Enter",
                        run: () => guide.showMeRequested(f.targetId) })
            list.push({ label: qsTr("Learn more"), detail: f.name, icon: Icons.book, tint: Theme.tintBlue,
                        run: () => guide.openDoc(f.docKey) })
        }
        list.push({ label: qsTr("Search features"), icon: Icons.search, tint: Theme.surfaceContentVariant, keys: "Ctrl F",
                    run: () => searchField.input.forceActiveFocus() })
        for (let i = 0; i < guideChapters.length; i++) {
            const key = guideChapters[i].key
            list.push({ label: qsTr("Read: %1").arg(guideChapters[i].label), icon: Icons.book, tint: Theme.tintBlue,
                        run: () => guide.openDoc(key) })
        }
        list.push({ label: qsTr("Setup walkthrough"), icon: Icons.compass, tint: Theme.tintTeal,
                    run: () => guide.restartWalkthroughRequested() })
        for (let a = 0; a < areaChips.length; a++) {
            const id = areaChips[a].id
            list.push({ label: qsTr("Show: %1").arg(areaChips[a].label), icon: Icons.apps, tint: Theme.surfaceContentVariant,
                        checked: selectedArea === id, run: () => { guide.selectedArea = id } })
        }
        list.push({ label: qsTr("Back to Home"), icon: Icons.back, tint: Theme.surfaceContentVariant,
                    run: () => { AppController.currentPage = "home" } })
        return list
    }

    Shortcut {
        sequences: [StandardKey.Find]
        enabled: guide.visible && !readerSheet.opened
        onActivated: searchField.input.forceActiveFocus()
    }


    function statusForFeature(item) {
        let _caps = AppController.capsRevision
        let _toggles = AppController.togglesRevision
        if (item.localAlways)
            return { label: qsTr("Ready on PC"), tone: "available", visible: true }
        if (item.wolCheck) {
            let st = AppController.wakeState
            if (st === "enabled")
                return { label: qsTr("Ready on PC"), tone: "available", visible: true }
            return { label: qsTr("Off in Settings"), tone: "off", visible: true }
        }
        if (item.localsendCheck) {
            return AppController.localsendEnabled
                ? { label: qsTr("Ready on PC"), tone: "available", visible: true }
                : { label: qsTr("Off in Settings"), tone: "off", visible: true }
        }
        if (item.prefKey && item.prefKey !== "") {
            let on = Boolean(Preferences[item.prefKey])
            return on
                ? { label: qsTr("Ready on PC"), tone: "available", visible: true }
                : { label: qsTr("Off in Settings"), tone: "off", visible: true }
        }
        if (item.pcLocal && item.toggleKey === "storage") {
            if (!guide.hasConnectedPhone)
                return { label: qsTr("Opt-in on Home"), tone: "off", visible: true }
        }
        if (!guide.hasConnectedPhone)
            return { label: "", tone: "neutral", visible: false }
        if (item.toggleKey && item.toggleKey !== "") {
            if (!guide.deviceTogglesMap[item.toggleKey])
                return { label: qsTr("Off on Home"), tone: "off", visible: true }
        }
        if (item.capKey && item.capKey !== "") {
            // The names AppController.featureState gives (bridge/app.rs).
            let st = AppController.featureState(guide.deviceId, item.capKey)
            let stateName = st ? (st.state || "") : ""
            let actionName = st ? (st.action || "") : ""
            let target = st ? (st.target || "") : ""
            if (stateName === "available")
                return { label: qsTr("Ready"), tone: "available", visible: true }
            if (stateName === "partial")
                return { label: qsTr("Basic mode"), tone: "basic", visible: true }
            if (stateName === "locked") {
                if (actionName === "enableToggle")
                    return { label: qsTr("Off on Home"), tone: "off", visible: true }
                if (actionName === "raisePower" && target === "elevated")
                    return { label: qsTr("Needs Elevated"), tone: "basic", visible: true }
                if (actionName === "raisePower")
                    return { label: qsTr("Needs Assist"), tone: "basic", visible: true }
                if (actionName === "updateApp")
                    return { label: qsTr("Needs an update"), tone: "warn", visible: true }
                return { label: qsTr("Needs phone permission"), tone: "warn", visible: true }
            }
            if (stateName === "unsupported")
                return { label: qsTr("Not on this phone"), tone: "off", visible: true }
            // Not known yet: nothing rather than a guess.
            return { label: "", tone: "neutral", visible: false }
        }
        return { label: qsTr("Ready"), tone: "available", visible: true }
    }

    // ---- Top: back, and search ----
    Item {
        id: topRow
        width: parent.width
        height: 52
        IconButton {
            id: backButton
            x: 12
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.back
            label: qsTr("Back to Home")
            onClicked: AppController.currentPage = "home"
        }
        Field {
            id: searchField
            anchors.left: backButton.right
            anchors.leftMargin: 8
            anchors.right: parent.right
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.search
            placeholder: qsTr("Search features")
            onTextChanged: guide.searchQuery = text
            onAccepted: guide.showSelected()
            Keys.onDownPressed: { guide.step(1); list.forceActiveFocus() }
            Keys.onEscapePressed: text = ""
        }
        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.line
        }
    }

    // ---- Left: the features ----
    Flickable {
        id: listView
        anchors.left: parent.left
        anchors.top: topRow.bottom
        anchors.bottom: parent.bottom
        width: Math.max(300, Math.min(440, guide.width * 0.4))
        contentHeight: list.height + 16
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        FocusScope {
            id: list
            x: 8
            y: 8
            width: listView.width - 16
            height: listCol.height
            activeFocusOnTab: true
            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_Down) {
                    guide.step(1)
                } else if (event.key === Qt.Key_Up) {
                    guide.step(-1)
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    guide.showSelected()
                } else {
                    return
                }
                event.accepted = true
            }

            Column {
                id: listCol
                width: parent.width
                spacing: 2

                Txt {
                    visible: !guide.hasConnectedPhone
                    x: Theme.rowPadding
                    width: parent.width - 2 * Theme.rowPadding
                    height: 30
                    text: qsTr("Connect a phone to see what's ready")
                    role: "bodySmall"
                    muted: true
                }
                Repeater {
                    model: guide.groups
                    delegate: Column {
                        id: group
                        required property var modelData
                        width: listCol.width
                        spacing: 2
                        SectionLabel {
                            width: parent.width
                            text: group.modelData.label
                        }
                        Repeater {
                            model: group.modelData.rows
                            delegate: ActionRow {
                                id: featureRow
                                required property var modelData
                                readonly property var status: guide.statusForFeature(modelData)
                                width: group.width
                                activeFocusOnTab: false
                                iconPath: modelData.icon
                                tint: guide.tintFor(modelData)
                                title: modelData.name
                                selected: guide.selectedFeature !== null && guide.selectedFeature.id === modelData.id
                                onClicked: {
                                    guide.selectedId = modelData.id
                                    list.forceActiveFocus()
                                }
                                Txt {
                                    visible: featureRow.status.visible && featureRow.status.label !== ""
                                    text: featureRow.status.label
                                    role: "bodySmall"
                                    color: guide.toneColor(featureRow.status.tone)
                                }
                            }
                        }
                    }
                }
                Txt {
                    visible: guide.orderedFeatures.length === 0
                    x: Theme.rowPadding
                    height: Theme.rowHeight
                    text: qsTr("Nothing matches")
                    muted: true
                }

                SectionLabel {
                    width: parent.width
                    text: qsTr("Handbook")
                }
                Repeater {
                    model: guide.guideChapters
                    delegate: ActionRow {
                        required property var modelData
                        width: listCol.width
                        activeFocusOnTab: false
                        iconPath: Icons.book
                        tint: Theme.tintBlue
                        title: modelData.label
                        onClicked: guide.openDoc(modelData.key)
                    }
                }
                ActionRow {
                    width: listCol.width
                    activeFocusOnTab: false
                    iconPath: Icons.compass
                    tint: Theme.tintTeal
                    title: qsTr("Setup walkthrough")
                    onClicked: guide.restartWalkthroughRequested()
                }
            }
        }
    }
    Rectangle {
        anchors.left: listView.right
        anchors.top: topRow.bottom
        anchors.bottom: parent.bottom
        width: 1
        color: Theme.line
    }

    // ---- Right: the feature picked ----
    Flickable {
        id: article
        anchors.left: listView.right
        anchors.leftMargin: 1
        anchors.right: parent.right
        anchors.top: topRow.bottom
        anchors.bottom: parent.bottom
        contentHeight: articleCol.height + 44
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: articleCol
            visible: guide.selectedFeature !== null
            x: 28
            y: 22
            width: Math.min(article.width - 56, 720)
            spacing: 16

            Row {
                spacing: 12
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 24; height: 24
                    path: guide.selectedFeature ? guide.selectedFeature.icon : ""
                    color: guide.selectedFeature ? guide.tintFor(guide.selectedFeature) : Theme.surfaceContentVariant
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: guide.selectedFeature ? guide.selectedFeature.name : ""
                    role: "headline"
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: guide.selectedStatus !== null && guide.selectedStatus.visible && guide.selectedStatus.label !== ""
                    text: guide.selectedStatus ? guide.selectedStatus.label : ""
                    role: "bodySmall"
                    color: guide.toneColor(guide.selectedStatus ? guide.selectedStatus.tone : "")
                }
            }
            Txt {
                width: Math.min(parent.width, 560)
                text: guide.selectedFeature ? guide.selectedFeature.summary : ""
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Grid {
                width: parent.width
                columns: 2
                columnSpacing: 16
                rowSpacing: 8
                Txt { width: 120; text: qsTr("Open"); muted: true }
                Txt {
                    width: parent.width - 136
                    text: guide.selectedFeature ? guide.selectedFeature.whereLabel : ""
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
                Txt { width: 120; text: qsTr("Needs"); muted: true }
                Txt {
                    width: parent.width - 136
                    text: guide.selectedFeature ? guide.selectedFeature.needsLabel : ""
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
                Txt {
                    width: 120
                    visible: guide.hasConnectedPhone
                    text: qsTr("Phone")
                    muted: true
                }
                Txt {
                    width: parent.width - 136
                    visible: guide.hasConnectedPhone
                    text: guide.deviceName
                }
            }
            Row {
                spacing: 8
                Button {
                    text: qsTr("Show me")
                    onClicked: guide.showSelected()
                }
                Button {
                    variant: "outline"
                    text: qsTr("Learn more")
                    iconPath: Icons.book
                    onClicked: if (guide.selectedFeature) guide.openDoc(guide.selectedFeature.docKey)
                }
            }
        }
    }

    // The handbook reader: a chapter at a time, with tabs for the others.
    Sheet {
        id: readerSheet
        cardWidth: 640
        // Not a binding: dismissing sets `opened`, which would break it, and
        // the next guide wouldn't open.
        Connections {
            target: AppController
            function onGuideDocOpenChanged() {
                if (AppController.guideDocOpen)
                    readerSheet.open()
                else
                    readerSheet.close()
            }
        }
        onOpenedChanged: {
            if (!opened && AppController.guideDocOpen)
                AppController.closeGuideDoc()
        }

        ColumnLayout {
            width: parent.width
            spacing: 14

            RowLayout {
                Layout.fillWidth: true
                spacing: 12

                Icon {
                    path: Icons.book
                    width: 20
                    height: 20
                    color: Theme.tintBlue
                }

                Txt {
                    Layout.fillWidth: true
                    text: AppController.guideDocTitle
                    role: "headline"
                    wrapMode: Text.WordWrap
                }

                IconButton {
                    iconPath: Icons.close
                    label: qsTr("Close guide")
                    onClicked: AppController.closeGuideDoc()
                }
            }

            // Topic tabs inside reader sheet
            Flow {
                Layout.fillWidth: true
                spacing: 6

                Repeater {
                    model: guide.guideChapters
                    delegate: TextAction {
                        required property var modelData
                        selected: guide.activeDocKey === modelData.key
                        text: modelData.label
                        Accessible.name: qsTr("Read %1").arg(modelData.label)
                        onClicked: guide.openDoc(modelData.key)
                    }
                }
            }

            Divider { Layout.fillWidth: true }

            Flickable {
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(380, docBodyText.implicitHeight + 16)
                contentWidth: width
                contentHeight: docBodyText.implicitHeight + 8
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                Txt {
                    id: docBodyText
                    width: parent.width
                    text: AppController.guideDocBody
                    role: "body"
                    muted: true
                    wrapMode: Text.WordWrap
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Item { Layout.fillWidth: true }
                Button {
                    text: qsTr("Close")
                    variant: "tonal"
                    size: "sm"
                    onClicked: AppController.closeGuideDoc()
                }
            }
        }
    }
}
