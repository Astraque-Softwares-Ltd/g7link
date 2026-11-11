// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Dialogs
import app.g7link

// Settings: a list of sections (each paired phone first, then General,
// Appearance, Notifications, Camera and sound, Files and sync, Shortcuts,
// Privacy, Connection and About) and the chosen section's rows.
Item {
    id: page
    property bool active: false
    property string deviceId
    property string deviceName
    property string currentSection: "devices"
    property int selectedDeviceIndex: DeviceList.count > 0
        ? Math.min(Math.max(0, AppController.currentDevice), DeviceList.count - 1) : 0
    signal pairRequested
    signal restartWalkthroughRequested

    readonly property bool wideLayout: page.width >= 680

    readonly property var categories: [
        { id: "general", title: qsTr("General") },
        { id: "appearance", title: qsTr("Appearance") },
        { id: "notifications", title: qsTr("Notifications") },
        { id: "camera_audio", title: qsTr("Camera and sound") },
        { id: "files_sync", title: qsTr("Files and sync") },
        { id: "shortcuts", title: qsTr("Shortcuts") },
        { id: "storage", title: qsTr("Privacy") },
        { id: "connection", title: qsTr("Connection") },
        { id: "about", title: qsTr("About") }
    ]
    readonly property string releaseUrl: "https://github.com/Astraque-Softwares-Ltd/g7link/releases/latest"
    // Matches the APK name on the GitHub release (see .github/workflows/release.yml).
    readonly property string adbCommand: "adb install -r G7Link-" + AppController.version + "-android.apk"
    readonly property string selectedDeviceId: DeviceList.count > 0 ? DeviceList.deviceIdAt(page.selectedDeviceIndex) : ""
    readonly property string selectedDeviceName: DeviceList.count > 0 ? DeviceList.deviceNameAt(page.selectedDeviceIndex) : ""

    // ---- Footer and Actions (Ctrl B) ----
    readonly property string footerLabel: qsTr("Settings · G7 Link %1").arg(AppController.version)
    readonly property var primaryAction: ({
        label: qsTr("Toggle"),
        keys: "Space",
        run: () => {
            const item = page.Window.activeFocusItem
            if (item && typeof item.toggled === "function" && item.checked !== undefined)
                item.toggled(!item.checked)
        }
    })
    readonly property var screenActions: {
        const list = []
        if (DeviceList.count > 0)
            list.push({ label: qsTr("Unpair %1").arg(page.selectedDeviceName), icon: Icons.unlink, tint: Theme.tintRed,
                        run: () => page.askUnpair(page.selectedDeviceId, page.selectedDeviceName) })
        list.push({ label: qsTr("Pair a phone"), icon: Icons.plus, tint: Theme.accent, run: () => page.pairRequested() })
        if (AppController.canUpdate)
            list.push({ label: AppController.updateVersion.length > 0 ? qsTr("Update to %1").arg(AppController.updateVersion)
                                                                        : qsTr("Check for updates"),
                        icon: Icons.refresh, tint: Theme.tintGreen, enabled: !AppController.updateBusy,
                        run: () => AppController.updateVersion.length > 0 ? AppController.installUpdate()
                                                                         : AppController.checkForUpdates() })
        list.push({ label: qsTr("Connection Doctor"), icon: Icons.doctor, tint: Theme.tintBlue, run: () => AppController.runDoctor() })
        list.push({ label: qsTr("Open logs"), icon: Icons.folder, tint: Theme.tintAmber,
                    run: () => Qt.openUrlExternally(AppController.logsUrl()) })
        list.push({ label: qsTr("Sync folders now"), icon: Icons.refresh, tint: Theme.tintAmber, run: () => Preferences.syncFoldersNow() })
        if (page.selectedDeviceId.length > 0)
            list.push({ label: qsTr("Add a synced folder"), icon: Icons.plus, tint: Theme.tintAmber,
                        run: () => { page.currentSection = "files_sync"; folderSyncSheet.openFor(page.selectedDeviceId) } })
        list.push({ label: qsTr("Test OBS connection"), icon: Icons.terminal, tint: Theme.tintGreen, run: () => Preferences.testObs() })
        list.push({ label: qsTr("Clear all history and caches"), icon: Icons.trash, tint: Theme.tintRed,
                    run: () => clearEverythingSheet.open() })
        list.push({ label: qsTr("Welcome screen"), icon: Icons.compass, tint: Theme.tintLavender,
                    run: () => page.restartWalkthroughRequested() })
        list.push({ label: qsTr("Features guide"), icon: Icons.book, tint: Theme.tintLavender,
                    run: () => { AppController.currentPage = "guide" } })
        list.push({ label: qsTr("Open the Android app's release page"), icon: Icons.openExternal, tint: Theme.tintAmber,
                    run: () => Qt.openUrlExternally(page.releaseUrl) })
        list.push({ label: qsTr("Copy the ADB install command"), icon: Icons.copy, tint: Theme.tintAmber,
                    run: () => AppController.copyText(page.adbCommand) })
        for (const c of page.categories) {
            const id = c.id
            list.push({ label: c.title, detail: qsTr("Section"), icon: Icons.chevronRight, tint: Theme.surfaceContentVariant,
                        run: () => { page.currentSection = id } })
        }
        return list
    }

    function askUnpair(id, name) {
        unpairSheet.deviceId = id
        unpairSheet.deviceName = name
        unpairSheet.open()
    }

    // Each setting's glyph draws in its own tint.
    function tintFor(path) {
        if (path === Icons.bell || path === Icons.messages || path === Icons.wifi || path === Icons.globe
            || path === Icons.signal || path === Icons.desktop || path === Icons.hotspot || path === Icons.bellOff
            || path === Icons.doctor)
            return Theme.tintBlue
        if (path === Icons.call || path === Icons.person || path === Icons.speaker || path === Icons.lock
            || path === Icons.deck || path === Icons.laptop)
            return Theme.tintTeal
        if (path === Icons.photo || path === Icons.camera || path === Icons.video)
            return Theme.tintCoral
        if (path === Icons.mirror || path === Icons.music || path === Icons.island || path === Icons.screenOff
            || path === Icons.phone || path === Icons.flip)
            return Theme.tintPink
        if (path === Icons.folder || path === Icons.send || path === Icons.download || path === Icons.copy
            || path === Icons.openExternal || path === Icons.lightbulb)
            return Theme.tintAmber
        if (path === Icons.battery || path === Icons.bolt || path === Icons.terminal || path === Icons.refresh
            || path === Icons.usb)
            return Theme.tintGreen
        if (path === Icons.mic || path === Icons.power || path === Icons.trash || path === Icons.soundOff
            || path === Icons.pause)
            return Theme.tintRed
        if (path.length > 0)
            return Theme.tintLavender
        return Theme.surfaceContentVariant
    }

    // A setting: its glyph in its tint, the title, a short muted detail
    // after it (or under it when it doesn't fit), and the control on the
    // right (or under the text when the control is wide).
    component SettingRow: Item {
        id: sr
        property string iconPath
        property string title
        property string description
        property bool stackBelow: false
        property color iconColor: page.tintFor(iconPath)
        default property alias trailing: srTrailing.data
        readonly property bool stackTrailing: stackBelow || (srTrailing.childrenRect.width > sr.width * 0.6 && sr.width > 0)
        readonly property real textX: srIcon.visible ? Theme.rowPadding + 30 : Theme.rowPadding
        readonly property real textRoom: Math.max(0, (stackTrailing ? sr.width - Theme.rowPadding
                                                                    : srTrailing.x - 12) - textX)
        readonly property bool inlineDetail: description.length === 0
            || srTitle.implicitWidth + 12 + srDetail.implicitWidth <= textRoom
        readonly property real textHeight: srTitle.height + (inlineDetail ? 0 : 2 + srDetail.height)
        readonly property real textY: stackTrailing ? 9 : Math.round((height - textHeight) / 2)

        implicitHeight: stackTrailing
            ? Math.max(Theme.rowHeight, textHeight + 18) + srTrailing.height + 8
            : Math.max(Theme.rowHeight, textHeight + 18, srTrailing.height + 8)

        Icon {
            id: srIcon
            visible: sr.iconPath.length > 0
            x: Theme.rowPadding
            y: sr.textY + Math.round((srTitle.lineHeight - height) / 2)
            width: 18; height: 18
            path: sr.iconPath
            color: sr.iconColor
        }
        Txt {
            id: srTitle
            x: sr.textX
            y: sr.textY
            width: Math.min(implicitWidth, sr.textRoom)
            text: sr.title
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
            maximumLineCount: 3
        }
        Txt {
            id: srDetail
            visible: sr.description.length > 0
            x: sr.inlineDetail ? srTitle.x + srTitle.width + 12 : sr.textX
            y: sr.inlineDetail ? sr.textY + 1 : sr.textY + srTitle.height + 2
            width: sr.inlineDetail ? Math.max(0, sr.textRoom - srTitle.width - 12) : sr.textRoom
            text: sr.description
            role: "bodySmall"
            muted: true
            wrapMode: sr.inlineDetail ? Text.NoWrap : Text.WordWrap
            elide: sr.inlineDetail ? Text.ElideRight : Text.ElideNone
        }
        Item {
            id: srTrailing
            x: sr.stackTrailing ? sr.textX : sr.width - width - Theme.rowPadding
            y: sr.stackTrailing ? sr.textY + sr.textHeight + 8 : Math.round((sr.height - height) / 2)
            width: childrenRect.width
            height: childrenRect.height
        }
    }

    // Rows that belong together, under an optional section label.
    component Group: Column {
        property string title
        width: parent ? parent.width : 0
        SectionLabel {
            visible: text.length > 0
            width: parent.width
            text: parent.title
        }
    }

    // A red text action (Unpair).
    component DangerAction: Rectangle {
        id: da
        property string text
        signal clicked
        implicitWidth: daText.implicitWidth + 16
        implicitHeight: 24
        radius: Theme.radiusSm
        color: daMouse.containsMouse ? Theme.raised : "transparent"
        border.width: Theme.focusVisible(da) ? 2 : 0
        border.color: Theme.focusRing
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: text
        Accessible.onPressAction: da.clicked()
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                da.clicked()
                event.accepted = true
            }
        }
        Txt {
            id: daText
            anchors.centerIn: parent
            text: da.text
            role: "bodySmall"
            color: Theme.tintRed
        }
        MouseArea {
            id: daMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: da.clicked()
        }
    }

    // The languages the app speaks besides the system's ({ code, name }).
    readonly property var languages: { try { return JSON.parse(Preferences.languages()) } catch (e) { return [] } }

    function syncSectionSideEffects() {
        Webcam.setPreviewVisible("settings", page.active && page.currentSection === "camera_audio"
                                 && page.Window.window !== null && page.Window.window.visible
                                 && page.Window.window.visibility !== Window.Minimized)
        if (page.active && (page.currentSection === "storage" || page.currentSection === "appearance"))
            AppController.refreshDataRetention()
    }

    function formatTrigger(t) {
        switch (t) {
        case "phone_connected": return qsTr("When phone connects")
        case "phone_disconnected": return qsTr("When phone disconnects")
        case "call_ringing": return qsTr("When phone rings")
        case "call_ended": return qsTr("When call ends")
        case "battery_low": return qsTr("When phone battery is low")
        case "battery_charged": return qsTr("When phone is charged")
        default: return (t || "").replace(/_/g, " ")
        }
    }

    function formatAction(a) {
        switch (a) {
        case "lock_pc": return qsTr("Lock this PC")
        case "sleep_pc": return qsTr("Sleep this PC")
        case "pause_pc_media":
        case "pause_media": return qsTr("Pause PC media")
        case "resume_pc_media":
        case "resume_media": return qsTr("Resume PC media")
        case "mute_pc": return qsTr("Mute PC audio")
        case "unmute_pc": return qsTr("Unmute PC audio")
        case "run_command": return qsTr("Run command")
        case "notify":
        case "show_toast": return qsTr("Show notification")
        default: return (a || "").replace(/_/g, " ")
        }
    }

    function formatKeyCombo(event) {
        if (event.key === Qt.Key_Control || event.key === Qt.Key_Shift
            || event.key === Qt.Key_Alt || event.key === Qt.Key_Meta
            || event.key === Qt.Key_unknown)
            return ""
        if (event.key === Qt.Key_Escape || event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab)
            return ""
        const mods = []
        if (event.modifiers & Qt.ControlModifier) mods.push("Ctrl")
        if (event.modifiers & Qt.AltModifier) mods.push("Alt")
        if (event.modifiers & Qt.ShiftModifier) mods.push("Shift")
        if (event.modifiers & Qt.MetaModifier) mods.push("Super")
        if (mods.length === 0)
            return ""
        let keyName = ""
        if (event.key === Qt.Key_Space) keyName = "Space"
        else if (event.key >= Qt.Key_A && event.key <= Qt.Key_Z)
            keyName = String.fromCharCode(event.key)
        else if (event.key >= Qt.Key_0 && event.key <= Qt.Key_9)
            keyName = String.fromCharCode(event.key)
        else if (event.key >= Qt.Key_F1 && event.key <= Qt.Key_F12)
            keyName = "F" + (event.key - Qt.Key_F1 + 1)
        if (keyName.length === 0)
            return ""
        return mods.join("+") + "+" + keyName
    }

    onActiveChanged: syncSectionSideEffects()
    onCurrentSectionChanged: {
        contentFlickable.contentY = 0
        syncSectionSideEffects()
        if (!Theme.reduceMotion) {
            column.opacity = 0.35
            sectionTranslate.y = 5
            Qt.callLater(() => sectionAnim.restart())
        } else {
            column.opacity = 1.0
            sectionTranslate.y = 0
        }
    }
    Component.onCompleted: {
        if (active)
            syncSectionSideEffects()
    }
    Component.onDestruction: Webcam.setPreviewVisible("settings", false)

    Connections {
        target: page.Window.window
        function onVisibilityChanged() { page.syncSectionSideEffects() }
    }

    Connections {
        target: DeviceList
        function onCountChanged() {
            if (page.selectedDeviceIndex >= DeviceList.count)
                page.selectedDeviceIndex = Math.max(0, DeviceList.count - 1)
        }
    }

    readonly property var retentionSummary: {
        try {
            return JSON.parse(AppController.dataRetentionSummary || "{}")
        } catch (e) {
            return ({})
        }
    }

    function formatRetentionBytes(bytes) {
        const b = Number(bytes || 0)
        const locale = Qt.locale()
        if (b <= 0)
            return qsTr("%1 B").arg(0)
        if (b < 1024)
            return qsTr("%1 B").arg(b)
        if (b < 1024 * 1024)
            return qsTr("%1 KB").arg(Math.max(1, Math.round(b / 1024)))
        if (b < 1024 * 1024 * 1024)
            return qsTr("%1 MB").arg(locale.toString(b / (1024 * 1024), "f", 1))
        if (b < 1024 * 1024 * 1024 * 1024)
            return qsTr("%1 GB").arg(locale.toString(b / (1024 * 1024 * 1024), "f", 1))
        return qsTr("%1 TB").arg(locale.toString(b / (1024 * 1024 * 1024 * 1024), "f", 1))
    }

    visible: active

    readonly property var backupPhones: {
        try { return JSON.parse(PhotoBackup.phones || "[]") } catch (e) { return [] }
    }
    readonly property string backupPhoneIds: backupPhones.map((p) => p.id).join(",")
    readonly property var toggleNames: ({
        notifications: qsTr("Notifications"),
        messages: qsTr("Messages"),
        calls: qsTr("Calls"),
        contacts: qsTr("Contacts"),
        clipboard: qsTr("Clipboard"),
        files: qsTr("Files"),
        recordings: qsTr("Voice recordings"),
        media: qsTr("Media controls"),
        photos: qsTr("Photos"),
        toggles: qsTr("Phone controls"),
        pc_actions: qsTr("Lock, sleep and wake"),
        mirroring: qsTr("Mirroring"),
        pc_mirror: qsTr("See this screen"),
        webcam: qsTr("Webcam"),
        mic: qsTr("Phone as microphone"),
        storage: qsTr("Phone storage in Files"),
        remote_input: qsTr("Control this PC"),
        commands: qsTr("Run Deck shell commands"),
        remote_files: qsTr("Browse PC folders")
    })
    readonly property var toggleDetails: ({
        notifications: qsTr("Show on this PC"),
        messages: qsTr("Read and reply"),
        calls: qsTr("Answer and dial"),
        contacts: qsTr("Names and numbers"),
        clipboard: qsTr("Both ways"),
        files: qsTr("Send and receive"),
        recordings: qsTr("Voice notes to this PC"),
        media: qsTr("Play, pause and skip"),
        photos: qsTr("Browse and save"),
        toggles: qsTr("Wi-Fi, Do not disturb, torch"),
        pc_actions: qsTr("From the phone"),
        mirroring: qsTr("Screen and app windows"),
        pc_mirror: qsTr("Mirror the PC to the phone"),
        webcam: qsTr("Use the phone camera"),
        mic: qsTr("Use the phone microphone"),
        storage: qsTr("Its folders in your file manager"),
        remote_input: qsTr("Mouse, keyboard and Deck"),
        commands: qsTr("Only for a phone you trust"),
        remote_files: qsTr("Your PC's folders on the phone")
    })
    readonly property var toggleTints: ({
        notifications: Theme.tintBlue,
        messages: Theme.tintBlue,
        calls: Theme.tintTeal,
        contacts: Theme.tintTeal,
        clipboard: Theme.tintLavender,
        files: Theme.tintAmber,
        recordings: Theme.tintRed,
        media: Theme.tintPink,
        photos: Theme.tintCoral,
        toggles: Theme.tintLavender,
        pc_actions: Theme.tintRed,
        mirroring: Theme.tintPink,
        pc_mirror: Theme.tintBlue,
        webcam: Theme.tintCoral,
        mic: Theme.tintRed,
        storage: Theme.tintAmber,
        remote_input: Theme.tintTeal,
        commands: Theme.tintGreen,
        remote_files: Theme.tintAmber
    })
    // Higher-trust access: off until the user turns it on.
    readonly property var optInKeys: ["remote_input", "remote_files", "pc_mirror", "pc_actions", "commands", "storage"]

    readonly property var toggleIcons: ({
        notifications: Icons.bell,
        messages: Icons.messages,
        calls: Icons.call,
        contacts: Icons.person,
        clipboard: Icons.clipboard,
        files: Icons.send,
        recordings: Icons.mic,
        media: Icons.music,
        photos: Icons.photo,
        toggles: Icons.toggles,
        pc_actions: Icons.lock,
        mirroring: Icons.mirror,
        pc_mirror: Icons.laptop,
        webcam: Icons.video,
        mic: Icons.mic,
        storage: Icons.folder,
        remote_input: Icons.mouse,
        commands: Icons.terminal,
        remote_files: Icons.folder
    })

    FolderDialog {
        id: photoBackupFolderPicker
        property string deviceId
        title: qsTr("Back up photos to")
        onAccepted: PhotoBackup.chooseFolder(deviceId, selectedFolder.toString())
    }

    FolderDialog {
        id: folderSyncPcPicker
        title: qsTr("Sync into")
        onAccepted: folderSyncSheet.pcFolder = Preferences.localFolder(selectedFolder.toString())
    }

    FolderDialog {
        id: recordingsFolderPicker
        title: qsTr("Save recordings in")
        onAccepted: Preferences.chooseRecordingsFolder(selectedFolder.toString())
    }

    // ---- Sections: each paired phone, then the rest ----
    Flickable {
        id: sectionList
        width: page.width >= 760 ? 200 : 168
        height: parent.height
        contentHeight: sectionCol.height + 16
        interactive: contentHeight > height
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: sectionCol
            x: 8
            y: 8
            width: parent.width - 16
            spacing: 2

            Repeater {
                model: DeviceList
                delegate: ActionRow {
                    required property int index
                    required property string name
                    readonly property bool current: page.currentSection === "devices" && page.selectedDeviceIndex === index
                    width: sectionCol.width
                    title: name
                    titleColor: current ? Theme.surfaceContent : Theme.surfaceContentVariant
                    selected: current
                    onClicked: {
                        page.selectedDeviceIndex = index
                        page.currentSection = "devices"
                    }
                }
            }
            ActionRow {
                visible: DeviceList.count === 0
                width: sectionCol.width
                title: qsTr("Phone")
                titleColor: page.currentSection === "devices" ? Theme.surfaceContent : Theme.surfaceContentVariant
                selected: page.currentSection === "devices"
                onClicked: page.currentSection = "devices"
            }
            Repeater {
                model: page.categories
                delegate: ActionRow {
                    required property var modelData
                    width: sectionCol.width
                    title: modelData.title
                    titleColor: selected ? Theme.surfaceContent : Theme.surfaceContentVariant
                    selected: page.currentSection === modelData.id
                    onClicked: page.currentSection = modelData.id
                }
            }
        }
    }
    Rectangle {
        x: sectionList.width
        width: 1
        height: parent.height
        color: Theme.line
    }

    // ---- The section ----
    Flickable {
        id: contentFlickable
        x: sectionList.width + 1
        width: parent.width - x
        height: parent.height
        contentHeight: column.height + 28
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: column
            x: 16
            y: 14
            width: Math.min(760, contentFlickable.width - 32)
            spacing: 14
            transform: Translate { id: sectionTranslate; y: 0 }

            ParallelAnimation {
                id: sectionAnim
                NumberAnimation {
                    target: column
                    property: "opacity"
                    from: 0.45
                    to: 1.0
                    duration: 120
                    easing.type: Easing.OutCubic
                }
                NumberAnimation {
                    target: sectionTranslate
                    property: "y"
                    from: 5
                    to: 0
                    duration: 135
                    easing.type: Easing.OutCubic
                }
            }

            // ================================================================
            // THE PHONE
            // ================================================================
            Column {
                visible: page.currentSection === "devices"
                width: visible ? parent.width : 0
                spacing: 14

                // No phone yet.
                Group {
                    visible: DeviceList.count === 0
                    SettingRow {
                        width: parent.width
                        iconPath: Icons.phone
                        iconColor: Theme.accent
                        title: qsTr("No phone paired")
                        Button {
                            size: "sm"
                            iconPath: Icons.plus
                            text: qsTr("Pair a phone")
                            onClicked: page.pairRequested()
                        }
                    }
                }

                // The chosen phone (only one shows at a time).
                Repeater {
                    model: page.currentSection === "devices" ? DeviceList : null
                    delegate: Column {
                        id: deviceCard
                        required property int index
                        required property string deviceId
                        required property string name
                        required property bool online
                        required property double pairedAt
                        required property string power
                        visible: index === page.selectedDeviceIndex
                        width: visible ? column.width : 0
                        spacing: 14

                        // Re-read when toggles (and so capabilities) change.
                        readonly property var toggles: AppController.capsRevision >= 0 && AppController.togglesRevision >= 0
                                                       ? AppController.deviceToggles(deviceId) : []
                        readonly property var canItems: toggles.filter((t) => page.optInKeys.indexOf(t.name) < 0)
                        readonly property var optInItems: page.optInKeys
                            .map((k) => toggles.find((t) => t.name === k))
                            .filter((t) => t !== undefined)
                        readonly property string powerText: power === "elevated" ? qsTr("Elevated")
                            : power === "assist" ? qsTr("Assist") : power === "basic" ? qsTr("Basic") : ""

                        Item {
                            width: parent.width
                            height: 32
                            Icon {
                                id: phoneGlyph
                                x: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                width: 20; height: 20
                                path: Icons.phone
                                color: Theme.accent
                            }
                            Txt {
                                id: phoneName
                                anchors.left: phoneGlyph.right
                                anchors.leftMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.min(implicitWidth, parent.width * 0.4)
                                text: deviceCard.name
                                weight: 500
                            }
                            Txt {
                                anchors.left: phoneName.right
                                anchors.leftMargin: 10
                                anchors.right: unpair.left
                                anchors.rightMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                role: "bodySmall"
                                muted: true
                                text: [
                                    deviceCard.pairedAt > 0
                                        ? qsTr("Paired %1").arg(new Date(deviceCard.pairedAt * 1000).toLocaleDateString(Qt.locale(), "d MMM"))
                                        : "",
                                    deviceCard.powerText,
                                    deviceCard.online ? "" : qsTr("Not connected")
                                ].filter((p) => p.length > 0).join(" · ")
                            }
                            DangerAction {
                                id: unpair
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("Unpair")
                                onClicked: page.askUnpair(deviceCard.deviceId, deviceCard.name)
                            }
                        }

                        Group {
                            title: qsTr("This phone can")
                            Repeater {
                                model: deviceCard.canItems
                                delegate: SettingRow {
                                    required property var modelData
                                    width: parent.width
                                    iconPath: page.toggleIcons[modelData.name] || ""
                                    iconColor: page.toggleTints[modelData.name] || Theme.surfaceContentVariant
                                    title: page.toggleNames[modelData.name] || modelData.name
                                    description: page.toggleDetails[modelData.name] || ""
                                    Toggle {
                                        label: page.toggleNames[modelData.name] || modelData.name
                                        checked: Boolean(modelData.on)
                                        onToggled: (on) => AppController.setDeviceToggle(deviceCard.deviceId, modelData.name, on)
                                    }
                                }
                            }
                        }

                        Group {
                            visible: deviceCard.optInItems.length > 0
                            title: qsTr("Off until you turn them on")
                            Repeater {
                                model: deviceCard.optInItems
                                delegate: SettingRow {
                                    required property var modelData
                                    width: parent.width
                                    iconPath: page.toggleIcons[modelData.name] || ""
                                    iconColor: page.toggleTints[modelData.name] || Theme.surfaceContentVariant
                                    title: page.toggleNames[modelData.name] || modelData.name
                                    description: page.toggleDetails[modelData.name] || ""
                                    Row {
                                        spacing: 8
                                        TextAction {
                                            visible: modelData.name === "storage" && Boolean(modelData.on)
                                            anchors.verticalCenter: parent.verticalCenter
                                            text: qsTr("Open")
                                            onClicked: AppController.openPhoneStorage(deviceCard.deviceId)
                                        }
                                        Toggle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            label: page.toggleNames[modelData.name] || modelData.name
                                            checked: Boolean(modelData.on)
                                            onToggled: (on) => AppController.setDeviceToggle(deviceCard.deviceId, modelData.name, on)
                                        }
                                    }
                                }
                            }
                        }

                        Group {
                            title: qsTr("On this PC")
                            // Instant Hotspot: the saved Wi-Fi network the PC joins
                            // after this phone turns its hotspot on.
                            SettingRow {
                                id: hotspotRow
                                width: parent.width
                                property string network: AppController.hotspotNetwork(deviceCard.deviceId)
                                iconPath: Icons.hotspot
                                title: qsTr("Join its hotspot")
                                description: hotspotRow.network.length > 0
                                    ? qsTr("Joins %1").arg(hotspotRow.network)
                                    : qsTr("Join it once from the system's Wi-Fi list first")
                                TextAction {
                                    text: qsTr("Choose network")
                                    onClicked: {
                                        hotspotSheet.deviceId = deviceCard.deviceId
                                        hotspotSheet.row = hotspotRow
                                        hotspotSheet.networks = JSON.parse(AppController.wifiNetworks() || "[]")
                                        hotspotSheet.open()
                                    }
                                }
                            }
                            SettingRow {
                                width: parent.width
                                iconPath: Icons.screenOff
                                title: qsTr("Keep its screen off")
                                description: qsTr("While mirroring · needs Elevated")
                                Toggle {
                                    label: qsTr("Keep phone screen off")
                                    checked: JSON.parse(Mirror.keepScreenOffPhones || "[]").indexOf(deviceCard.deviceId) >= 0
                                    onToggled: (on) => Mirror.setKeepScreenOff(deviceCard.deviceId, on)
                                }
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "appearance"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.palette
                            title: qsTr("Theme")
                            Segmented {
                                options: [
                                    { value: "quiet", label: "G7" },
                                    { value: "hive", label: qsTr("Hive") },
                                    { value: "bloom", label: qsTr("Bloom") },
                                    { value: "glass", label: qsTr("Glass") },
                                    { value: "graphite", label: qsTr("Graphite") }
                                ]
                                value: Theme.hive ? "hive" : (Theme.glass ? "glass" : Preferences.theme)
                                onPicked: (value) => Preferences.theme = value
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.moon
                            title: qsTr("Mode")
                            Segmented {
                                options: [
                                    { value: "system", label: qsTr("System") },
                                    { value: "light", label: qsTr("Light") },
                                    { value: "dark", label: qsTr("Dark") }
                                ]
                                value: Preferences.colorMode
                                onPicked: (value) => Preferences.colorMode = value
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.text
                            title: qsTr("App size")
                            description: qsTr("After a restart")
                            Segmented {
                                options: [
                                    { value: 100, label: "100%" },
                                    { value: 110, label: "110%" },
                                    { value: 125, label: "125%" },
                                    { value: 150, label: "150%" }
                                ]
                                value: Preferences.uiScale
                                onPicked: (value) => Preferences.uiScale = value
                            }
                        }
                        // Hive's colors are fixed: no seed to pick.
                        SettingRow {
                            id: colorRow
                            width: parent.width
                            visible: !Theme.graphite && !Theme.hive && !Theme.quiet
                            stackBelow: true
                            iconPath: Icons.colorSwatch
                            title: qsTr("Color")
                            Column {
                                id: colorControl
                                width: Math.max(
                                    swatchesRow.implicitWidth,
                                    phonePillsRow.implicitWidth
                                )
                                spacing: 8

                                Row {
                                    id: swatchesRow
                                    anchors.right: colorRow.stackTrailing ? undefined : parent.right
                                    spacing: 8

                                    // Wallpaper swatch
                                    Rectangle {
                                        id: wallpaperSwatch
                                        readonly property bool selected: Preferences.seed === "wallpaper"
                                        readonly property var wallpaperPalette: Theme.wallpaperSeed
                                            ? Theme.wallpaperSeed[Theme.dark ? "dark" : "light"] : null
                                        width: 28; height: 28; radius: 14
                                        activeFocusOnTab: true
                                        color: wallpaperPalette ? wallpaperPalette.primary : Theme.surfaceContainerHighest
                                        border.width: selected || Theme.focusVisible(wallpaperSwatch) ? 2 : 0
                                        border.color: Theme.focusVisible(wallpaperSwatch) ? Theme.primary : Theme.surfaceContent
                                        Accessible.role: Accessible.RadioButton
                                        Accessible.name: qsTr("Wallpaper")
                                        Accessible.checked: selected
                                        Accessible.onPressAction: Preferences.seed = "wallpaper"
                                        Keys.onPressed: (event) => {
                                            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                                Preferences.seed = "wallpaper"
                                                event.accepted = true
                                            }
                                        }
                                        Icon {
                                            anchors.centerIn: parent
                                            width: 14; height: 14
                                            stroke: 2
                                            path: Icons.photo
                                            color: wallpaperSwatch.wallpaperPalette ? wallpaperSwatch.wallpaperPalette.onPrimary : Theme.surfaceContent
                                        }
                                        HoverHandler { cursorShape: Qt.PointingHandCursor }
                                        TapHandler { onTapped: Preferences.seed = "wallpaper" }
                                    }

                                    // Preset seeds
                                    Repeater {
                                        model: Object.keys(Tokens.data.themes.bloom.seeds)
                                        delegate: Rectangle {
                                            id: swatch
                                            required property string modelData
                                            readonly property bool selected: Preferences.seed === modelData
                                                || (!Theme.isPhoneSeed && !Theme.isArtworkSeed && Preferences.seed !== "wallpaper" && Theme.seed === modelData)
                                            width: 28; height: 28; radius: 14
                                            activeFocusOnTab: true
                                            color: Tokens.data.themes.bloom.seeds[modelData].seed
                                            border.width: selected || Theme.focusVisible(swatch) ? 2 : 0
                                            border.color: Theme.focusVisible(swatch) ? Theme.primary : Theme.surfaceContent
                                            Accessible.role: Accessible.RadioButton
                                            Accessible.name: modelData.charAt(0).toUpperCase() + modelData.slice(1)
                                            Accessible.checked: selected
                                            Accessible.onPressAction: Preferences.seed = modelData
                                            Keys.onPressed: (event) => {
                                                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                                    Preferences.seed = swatch.modelData
                                                    event.accepted = true
                                                }
                                            }
                                            Icon {
                                                anchors.centerIn: parent
                                                width: 14; height: 14
                                                stroke: 2
                                                path: Icons.check
                                                color: "white"
                                                visible: swatch.selected
                                            }
                                            HoverHandler { cursorShape: Qt.PointingHandCursor }
                                            TapHandler { onTapped: Preferences.seed = swatch.modelData }
                                        }
                                    }

                                    // Compact inline AMOLED toggle shown inside the Color row when Black seed is selected
                                    Rectangle {
                                        id: inlineAmoledPill
                                        visible: !Theme.graphite && Preferences.seed === "black"
                                        height: 28
                                        width: inlineAmoledRow.implicitWidth + 16
                                        radius: 14
                                        color: Theme.glass ? Theme.tileColor : Theme.surfaceContainerHigh
                                        border.width: Theme.glass || Preferences.amoled ? 1 : 0
                                        border.color: Preferences.amoled ? Theme.primary : Theme.cardBorder

                                        Row {
                                            id: inlineAmoledRow
                                            anchors.centerIn: parent
                                            spacing: 6

                                            Txt {
                                                anchors.verticalCenter: parent.verticalCenter
                                                role: "caption"
                                                weight: 600
                                                text: qsTr("AMOLED")
                                                color: Preferences.amoled ? Theme.primary : Theme.surfaceContentVariant
                                            }

                                            Toggle {
                                                anchors.verticalCenter: parent.verticalCenter
                                                scale: 0.78
                                                checked: Preferences.amoled
                                                label: qsTr("Pure black (AMOLED)")
                                                onToggled: (on) => Preferences.amoled = on
                                            }
                                        }
                                    }
                                }

                                // Dynamic pills row: Playing media artwork + paired phone color pills
                                Row {
                                    id: phonePillsRow
                                    anchors.right: colorRow.stackTrailing ? undefined : parent.right
                                    spacing: 8

                                    // Media artwork dynamic sync pill
                                    Rectangle {
                                        id: artworkSwatch
                                        readonly property bool selected: Theme.isArtworkSeed
                                        readonly property var artworkPalette: Theme.artworkSeed
                                            ? Theme.artworkSeed[Theme.dark ? "dark" : "light"] : null
                                        height: 28
                                        width: artworkSwatchRow.implicitWidth + 14
                                        radius: Theme.pill(height)
                                        activeFocusOnTab: true
                                        color: selected ? Theme.secondaryContainer : Theme.surfaceContainerHigh
                                        border.width: selected || Theme.focusVisible(artworkSwatch) ? 2 : 0
                                        border.color: Theme.focusVisible(artworkSwatch) ? Theme.primary : Theme.surfaceContent
                                        Accessible.role: Accessible.RadioButton
                                        Accessible.name: qsTr("Media artwork")
                                        Accessible.checked: selected
                                        Accessible.onPressAction: Preferences.seed = "artwork"
                                        Keys.onPressed: (event) => {
                                            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                                Preferences.seed = "artwork"
                                                event.accepted = true
                                            }
                                        }
                                        Row {
                                            id: artworkSwatchRow
                                            anchors.centerIn: parent
                                            spacing: 6
                                            Rectangle {
                                                anchors.verticalCenter: parent.verticalCenter
                                                width: 18; height: 18; radius: 9
                                                color: Theme.artworkSeed && Theme.artworkSeed.seed
                                                    ? Theme.artworkSeed.seed
                                                    : (artworkSwatch.artworkPalette ? artworkSwatch.artworkPalette.primary : Theme.primary)
                                                Icon {
                                                    anchors.centerIn: parent
                                                    width: 11; height: 11
                                                    stroke: 2
                                                    path: Icons.music
                                                    color: "white"
                                                }
                                            }
                                            Txt {
                                                anchors.verticalCenter: parent.verticalCenter
                                                role: "label"
                                                size: 12
                                                text: qsTr("Media artwork")
                                                color: artworkSwatch.selected ? Theme.secondaryContainerContent : Theme.surfaceContent
                                            }
                                        }
                                        HoverHandler { cursorShape: Qt.PointingHandCursor }
                                        TapHandler { onTapped: Preferences.seed = "artwork" }
                                    }

                                    Repeater {
                                        model: Theme.phoneSeeds
                                        delegate: Rectangle {
                                            id: phoneSwatch
                                            required property var modelData
                                            required property int index
                                            readonly property bool selected: Preferences.seed === modelData.key
                                                || (Preferences.seed === "phone" && index === 0)
                                                || (modelData.id.length > 0 && Preferences.seed === "phone:" + modelData.id)
                                            readonly property var phonePalette: modelData.scheme
                                                ? modelData.scheme[Theme.dark ? "dark" : "light"] : null
                                            height: 28
                                            width: phoneSwatchRow.implicitWidth + 14
                                            radius: Theme.pill(height)
                                            activeFocusOnTab: true
                                            color: selected ? Theme.secondaryContainer : Theme.surfaceContainerHigh
                                            border.width: selected || Theme.focusVisible(phoneSwatch) ? 2 : 0
                                            border.color: Theme.focusVisible(phoneSwatch) ? Theme.primary : Theme.surfaceContent
                                            Accessible.role: Accessible.RadioButton
                                            Accessible.name: modelData.label
                                            Accessible.checked: selected
                                            Accessible.onPressAction: Preferences.seed = modelData.key
                                            Keys.onPressed: (event) => {
                                                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                                    Preferences.seed = phoneSwatch.modelData.key
                                                    event.accepted = true
                                                }
                                            }
                                            Row {
                                                id: phoneSwatchRow
                                                anchors.centerIn: parent
                                                spacing: 6
                                                Rectangle {
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    width: 18; height: 18; radius: 9
                                                    color: phoneSwatch.modelData.scheme && phoneSwatch.modelData.scheme.seed
                                                        ? phoneSwatch.modelData.scheme.seed
                                                        : (phoneSwatch.phonePalette ? phoneSwatch.phonePalette.primary : Theme.primary)
                                                    Icon {
                                                        anchors.centerIn: parent
                                                        width: 11; height: 11
                                                        stroke: 2
                                                        path: Icons.phone
                                                        color: "white"
                                                    }
                                                }
                                                Txt {
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    role: "label"
                                                    size: 12
                                                    text: phoneSwatch.modelData.label
                                                    color: phoneSwatch.selected ? Theme.secondaryContainerContent : Theme.surfaceContent
                                                }
                                            }
                                            HoverHandler { cursorShape: Qt.PointingHandCursor }
                                            TapHandler { onTapped: Preferences.seed = phoneSwatch.modelData.key }
                                        }
                                    }
                                }
                            }
                        }

                        // Hive: the comb's motion (Hive keeps its own colors).
                        SettingRow {
                            width: parent.width
                            visible: Theme.hive
                            iconPath: Icons.sparkle
                            title: qsTr("Animated comb")
                            description: AppController.reduceMotion
                                ? (qsTr("Still while animations are off in system settings."))
                                : (AppController.powerSaver
                                    ? qsTr("Still while energy saver is on.")
                                    : qsTr("The honeycomb breathes and pulses."))
                            Toggle {
                                label: qsTr("Animated comb")
                                checked: Preferences.combMotion
                                onToggled: (on) => Preferences.combMotion = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            visible: !Theme.hive && Theme.graphite && Theme.dark
                            iconPath: Icons.moon
                            title: qsTr("Pure black (AMOLED)")
                            Toggle {
                                checked: Preferences.amoled
                                label: qsTr("Pure black (AMOLED)")
                                onToggled: (on) => Preferences.amoled = on
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "general"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width

                        // Language: the system's, English, or another one the app
                        // speaks, picked from a list (each named in its own language).
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.globe
                            title: qsTr("Language")
                            Segmented {
                                readonly property var other: page.languages.find(
                                    (l) => l.code === Preferences.language && l.code !== "en")
                                options: [
                                    { value: "", label: qsTr("System") },
                                    { value: "en", label: "English" },
                                    other ? { value: other.code, label: other.name }
                                          : { value: "more", label: qsTr("More") }
                                ]
                                value: Preferences.language
                                onPicked: (v) => {
                                    if (v === "" || v === "en")
                                        Preferences.language = v
                                    else
                                        languageSheet.open()
                                }
                            }
                        }

                        SettingRow {
                            id: startWinRow
                            width: parent.width
                            iconPath: Icons.power
                            title: qsTr("Start at login")
                            description: qsTr("In the tray")
                            Toggle {
                                label: qsTr("Start at login")
                                checked: Preferences.startWithWindows
                                onToggled: (on) => Preferences.startWithWindows = on
                            }
                        }
                        SettingRow {
                            id: stayTrayRow
                            width: parent.width
                            iconPath: Icons.tray
                            title: qsTr("Stay in system tray")
                            description: qsTr("Keep running when closed")
                            Toggle {
                                label: qsTr("Stay in system tray")
                                checked: Preferences.closeToTray
                                onToggled: (on) => Preferences.closeToTray = on
                            }
                        }

                        SettingRow {
                            id: islandRow
                            width: parent.width
                            iconPath: Icons.island
                            title: qsTr("G7 Island")
                            description: qsTr("Calls, transfers and media at the top")
                            Toggle {
                                label: qsTr("G7 Island")
                                checked: Preferences.g7Island
                                onToggled: (on) => Preferences.g7Island = on
                            }
                        }
                        SettingRow {
                            id: batteryAlertsRow
                            width: parent.width
                            iconPath: Icons.battery
                            title: qsTr("Battery alerts")
                            description: qsTr("Low and full")
                            Toggle {
                                label: qsTr("Battery alerts")
                                checked: Preferences.batteryAlerts
                                onToggled: (on) => Preferences.batteryAlerts = on
                            }
                        }

                        // When the Island shows.
                        Repeater {
                            model: Preferences.g7Island ? [
                                { key: "calls", label: qsTr("Island: calls"), icon: Icons.call },
                                { key: "transfers", label: qsTr("Island: transfers"), icon: Icons.send },
                                { key: "media", label: qsTr("Island: media"), icon: Icons.music },
                                { key: "notifications", label: qsTr("Island: notifications"), icon: Icons.bell }
                            ] : []
                            delegate: SettingRow {
                                id: trigRow
                                required property var modelData
                                readonly property bool on: {
                                    if (modelData.key === "calls") return Preferences.g7IslandCalls
                                    if (modelData.key === "transfers") return Preferences.g7IslandTransfers
                                    if (modelData.key === "media") return Preferences.g7IslandMedia
                                    return Preferences.g7IslandNotifications
                                }
                                width: parent.width
                                iconPath: modelData.icon
                                title: modelData.label
                                Toggle {
                                    label: trigRow.modelData.label
                                    checked: trigRow.on
                                    onToggled: (next) => {
                                        if (trigRow.modelData.key === "calls") Preferences.g7IslandCalls = next
                                        else if (trigRow.modelData.key === "transfers") Preferences.g7IslandTransfers = next
                                        else if (trigRow.modelData.key === "media") Preferences.g7IslandMedia = next
                                        else Preferences.g7IslandNotifications = next
                                    }
                                }
                            }
                        }

                        Column {
                            id: batteryThresholdBlock
                            visible: Preferences.batteryAlerts
                            width: parent.width
                            spacing: 0

                            SettingRow {
                                width: parent.width
                                iconPath: Icons.bolt
                                title: qsTr("Full charge target")
                                Segmented {
                                    options: [
                                        { value: 100, label: "100%" },
                                        { value: 80, label: qsTr("80% · protect battery") }
                                    ]
                                    value: Preferences.batteryFullThreshold <= 80 ? 80 : 100
                                    onPicked: (val) => Preferences.batteryFullThreshold = val
                                }
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.keyboard
                            title: qsTr("Enter sends messages")
                            description: qsTr("In mirroring · Ctrl+Enter for a new line · needs Assist")
                            Toggle {
                                label: qsTr("Enter sends messages")
                                checked: Preferences.enterSends
                                onToggled: (on) => Preferences.enterSends = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.clipboard
                            title: qsTr("Sync clipboard to phone")
                            description: qsTr("Text and images")
                            Toggle {
                                label: qsTr("Sync clipboard to phone")
                                checked: Preferences.autoClipboard
                                onToggled: (on) => Preferences.autoClipboard = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.clipboard
                            title: qsTr("Clipboard follows control")
                            description: qsTr("When you start controlling the phone")
                            Toggle {
                                label: qsTr("Clipboard follows control")
                                checked: Preferences.flowFollowClipboard
                                onToggled: (on) => Preferences.flowFollowClipboard = on
                            }
                        }

                        SettingRow {
                            id: copyOtpRow
                            width: parent.width
                            iconPath: Icons.lock
                            title: qsTr("Copy one-time codes")
                            description: qsTr("From texts")
                            Toggle {
                                label: qsTr("Copy one-time codes")
                                checked: Preferences.autoCopyOtp
                                onToggled: (on) => Preferences.autoCopyOtp = on
                            }
                        }
                        SettingRow {
                            id: copyFilesRow
                            width: parent.width
                            iconPath: Icons.copy
                            title: qsTr("Copy received files")
                            description: qsTr("Ready to paste")
                            Toggle {
                                label: qsTr("Copy received files")
                                checked: Preferences.clipboardAutoCopyReceivedFiles
                                onToggled: (on) => Preferences.clipboardAutoCopyReceivedFiles = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.lightbulb
                            title: qsTr("Suggest clipboard actions")
                            description: qsTr("Links, maps, numbers and emails")
                            Toggle {
                                label: qsTr("Suggest clipboard actions")
                                checked: Preferences.suggestClipboardActions
                                onToggled: (on) => Preferences.suggestClipboardActions = on
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "camera_audio"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    SettingRow {
                        width: parent.width
                        iconPath: Icons.mirror
                        title: qsTr("Mirror quality")
                        description: Preferences.mirrorQuality === "sharper"
                            ? qsTr("Full resolution · needs good Wi-Fi or USB")
                            : Preferences.mirrorQuality === "smoother"
                            ? qsTr("Holds up on weak Wi-Fi")
                            : qsTr("From the next time you mirror")
                        Segmented {
                            options: [
                                { value: "balanced", label: qsTr("Balanced") },
                                { value: "sharper", label: qsTr("Sharper") },
                                { value: "smoother", label: qsTr("Smoother") }
                            ]
                            value: Preferences.mirrorQuality
                            onPicked: (value) => Preferences.mirrorQuality = value
                        }
                    }
                }

                Group {
                    width: parent.width
                    Column {
                        width: parent.width

                        SettingRow {
                            id: streamAudioRow
                            width: parent.width
                            iconPath: Icons.speaker
                            title: qsTr("Stream PC audio to phone")
                            Toggle {
                                label: qsTr("Stream PC audio to phone")
                                checked: Preferences.streamPcAudioToPhone
                                onToggled: (on) => Preferences.streamPcAudioToPhone = on
                            }
                        }
                        SettingRow {
                            id: pauseOnCallRow
                            width: parent.width
                            iconPath: Icons.pause
                            title: qsTr("Pause media during calls")
                            description: qsTr("When the phone rings")
                            Toggle {
                                label: qsTr("Pause media during calls")
                                checked: Preferences.pauseMediaOnCall
                                onToggled: (on) => Preferences.pauseMediaOnCall = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.soundOff
                            title: qsTr("Mute this PC while a phone watches")
                            description: qsTr("The phone still plays it")
                            Toggle {
                                label: qsTr("Mute this PC while a phone watches")
                                checked: Preferences.muteWhileWatched
                                onToggled: (on) => Preferences.muteWhileWatched = on
                            }
                        }

                        // Phone as microphone: its sound plays into an output; a
                        // virtual audio cable's other end is a microphone apps can pick.
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.mic
                            title: qsTr("Phone as microphone")
                            description: PhoneMic.phase === "live"
                                ? qsTr("Live from %1, playing into %2.").arg(PhoneMic.deviceName).arg(PhoneMic.outputName)
                                : PhoneMic.phase === "asking"
                                    ? qsTr("Tap Start on %1.").arg(PhoneMic.deviceName)
                                    : PhoneMic.cable
                                        ? qsTr("Plays into %1, so apps can pick its other end as a microphone.").arg(PhoneMic.outputName)
                                        : qsTr("Plays into %1").arg(PhoneMic.outputName)
                            Row {
                                spacing: 8
                                Button {
                                    variant: "outline"
                                    size: "sm"
                                    text: qsTr("Output")
                                    onClicked: {
                                        PhoneMic.refresh()
                                        micOutputSheet.open()
                                    }
                                }
                                Button {
                                    size: "sm"
                                    variant: PhoneMic.phase === "off" ? "fill" : "tonal"
                                    enabled: PhoneMic.phase !== "off" || PhoneMic.device !== ""
                                    text: PhoneMic.phase === "off" ? qsTr("Start") : qsTr("Stop")
                                    onClicked: PhoneMic.phase === "off" ? PhoneMic.start(PhoneMic.device) : PhoneMic.stop()
                                }
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.mic
                            iconColor: Theme.tintTeal
                            title: qsTr("Microphone volume")
                            description: qsTr("%1%").arg(PhoneMic.gain)
                            ValueSlider {
                                width: 200
                                from: PhoneMic.minGain
                                to: PhoneMic.maxGain
                                step: 10
                                value: PhoneMic.gain
                                label: qsTr("Microphone volume")
                                valueText: qsTr("%1%").arg(PhoneMic.gain)
                                onMoved: (v) => PhoneMic.setLoudness(v)
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.video
                            title: qsTr("Virtual camera add-on")
                            description: Webcam.addonRegistered
                                ? qsTr("Ready for video calls")
                                : qsTr("Loads v4l2loopback · asks for your password")
                            Row {
                                spacing: 12
                                Spinner {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Webcam.addonBusy
                                }
                                Row {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Webcam.addonRegistered
                                    spacing: 6
                                    StatusDot {
                                        anchors.verticalCenter: parent.verticalCenter
                                        online: true
                                    }
                                    Txt {
                                        anchors.verticalCenter: parent.verticalCenter
                                        role: "bodySmall"
                                        muted: true
                                        text: qsTr("Ready")
                                    }
                                }
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    variant: Webcam.addonRegistered ? "outline" : "tonal"
                                    size: "sm"
                                    enabled: !Webcam.addonBusy
                                    text: Webcam.addonRegistered ? qsTr("Remove") : qsTr("Set up")
                                    onClicked: Webcam.addonRegistered ? Webcam.removeAddon() : Webcam.setupAddon()
                                }
                            }
                        }

                        SettingRow {
                            width: parent.width
                            visible: DeviceList.count > 1
                            iconPath: Icons.phone
                            title: qsTr("Phone")
                            Row {
                                spacing: 8
                                Repeater {
                                    model: DeviceList
                                    delegate: Chip {
                                        required property string deviceId
                                        required property string name
                                        interactive: true
                                        text: name
                                        selected: Webcam.selectedPhone === deviceId
                                        onClicked: Webcam.selectPhone(deviceId)
                                    }
                                }
                            }
                        }

                        SettingRow {
                            id: cameraSelectRow
                            width: parent.width
                            iconPath: Icons.camera
                            title: qsTr("Camera")
                            Segmented {
                                options: [
                                    { value: "back", label: qsTr("Back") },
                                    { value: "front", label: qsTr("Front") }
                                ]
                                value: Webcam.camera
                                onPicked: (value) => Webcam.setCamera(value)
                            }
                        }
                        SettingRow {
                            id: mirrorCamRow
                            width: parent.width
                            iconPath: Icons.flip
                            title: qsTr("Mirror image")
                            Toggle {
                                label: qsTr("Mirror image")
                                checked: Webcam.mirror
                                onToggled: (on) => Webcam.setMirrorImage(on)
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.camera
                            title: qsTr("Resolution")
                            Segmented {
                                // 4K only when the phone's camera in use can stream it.
                                options: [
                                    { value: "720", label: "720p" },
                                    { value: "1080", label: "1080p" }
                                ].concat(Webcam.uhd ? [{ value: "2160", label: "4K" }] : [])
                                value: String(Webcam.height)
                                onPicked: (value) => Webcam.setResolution(Number(value))
                            }
                        }

                        Column {
                            width: parent.width
                            spacing: 10
                            topPadding: 8
                            Item {
                                width: parent.width
                                height: Math.max(webcamStatusLabel.height, webcamStartBtn.height)
                                Txt {
                                    id: webcamStatusLabel
                                    anchors.left: parent.left
                                    anchors.right: webcamStartBtn.left
                                    anchors.rightMargin: 12
                                    anchors.verticalCenter: parent.verticalCenter
                                    role: "bodySmall"
                                    muted: Webcam.phase !== "streaming"
                                    elide: Text.ElideRight
                                    text: Webcam.statusText.length > 0
                                        ? Webcam.statusText
                                        : (Webcam.selectedPhoneName.length > 0
                                           ? qsTr("Preview · %1").arg(Webcam.selectedPhoneName)
                                           : qsTr("Pair a phone to use its camera as a webcam"))
                                }
                                Button {
                                    id: webcamStartBtn
                                    anchors.right: parent.right
                                    anchors.verticalCenter: parent.verticalCenter
                                    enabled: DeviceList.count > 0
                                    variant: Webcam.phase === "streaming" || Webcam.phase === "asking" ? "outline" : "tonal"
                                    size: "sm"
                                    iconPath: Webcam.phase === "streaming" || Webcam.phase === "asking" ? Icons.close : Icons.video
                                    text: Webcam.phase === "streaming" || Webcam.phase === "asking"
                                        ? qsTr("Stop webcam")
                                        : qsTr("Start webcam")
                                    onClicked: {
                                        if (Webcam.phase === "streaming" || Webcam.phase === "asking")
                                            Webcam.stop()
                                        else
                                            Webcam.start("")
                                    }
                                }
                            }
                            Rectangle {
                                width: parent.width
                                height: Math.round(width * 9 / 16)
                                radius: Theme.radiusMd
                                color: Theme.raised
                                border.width: 1
                                border.color: Theme.outlineVariant
                                clip: true
                                VideoView {
                                    anchors.fill: parent
                                    stream: "webcam"
                                }
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "notifications"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.bellOff
                            title: qsTr("Quiet while this PC is locked")
                            description: qsTr("Still collected on Home")
                            Toggle {
                                label: qsTr("Quiet while this PC is locked")
                                checked: Preferences.quietWhenLocked
                                onToggled: (on) => Preferences.quietWhenLocked = on
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.lock
                            title: qsTr("Don't receive while this PC is locked")
                            description: qsTr("Until you unlock")
                            Toggle {
                                label: qsTr("Don't receive while this PC is locked")
                                checked: Preferences.holdWhenLocked
                                onToggled: (on) => Preferences.holdWhenLocked = on
                            }
                        }

                        SettingRow {
                            id: syncDndRow
                            width: parent.width
                            iconPath: Icons.moon
                            title: qsTr("Follow phone Do not disturb")
                            Toggle {
                                label: qsTr("Follow phone Do not disturb")
                                checked: Preferences.syncDnd
                                onToggled: (on) => Preferences.syncDnd = on
                            }
                        }
                        SettingRow {
                            id: forwardPcNotifRow
                            width: parent.width
                            iconPath: Icons.bell
                            title: qsTr("Forward PC notifications")
                            description: qsTr("While you're away")
                            Toggle {
                                label: qsTr("Forward PC notifications")
                                checked: Preferences.forwardPcNotificationsToPhone
                                onToggled: (on) => Preferences.forwardPcNotificationsToPhone = on
                            }
                        }

                        Repeater {
                            model: { try { return JSON.parse(NotificationList.apps) } catch (e) { return [] } }
                            delegate: Column {
                                id: appRow
                                required property var modelData
                                width: parent.width
                                Item {
                                    width: parent.width
                                    height: 54
                                    Item {
                                        id: appIcon
                                        x: 4
                                        anchors.verticalCenter: parent.verticalCenter
                                        width: 28; height: 28
                                        Image {
                                            id: appImage
                                            anchors.fill: parent
                                            source: appRow.modelData.icon
                                            sourceSize: Qt.size(56, 56)
                                            mipmap: true
                                            visible: status === Image.Ready
                                        }
                                        Avatar { anchors.fill: parent; visible: appImage.status !== Image.Ready; name: appRow.modelData.name }
                                    }
                                    Txt {
                                        anchors.left: appIcon.right
                                        anchors.leftMargin: 14
                                        anchors.right: appChoice.left
                                        anchors.rightMargin: 12
                                        anchors.verticalCenter: parent.verticalCenter
                                        role: "body"
                                        text: appRow.modelData.name
                                        elide: Text.ElideRight
                                    }
                                    Segmented {
                                        id: appChoice
                                        anchors.right: parent.right
                                        anchors.verticalCenter: parent.verticalCenter
                                        options: [
                                            { value: "show", label: qsTr("Show") },
                                            { value: "quiet", label: qsTr("No pop-ups") },
                                            { value: "hidden", label: qsTr("Hide") }
                                        ]
                                        value: appRow.modelData.rule
                                        onPicked: (value) => NotificationList.setAppRule(appRow.modelData.app, value)
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "files_sync"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width

                        SettingRow {
                            id: sendToMenuRow
                            width: parent.width
                            iconPath: Icons.send
                            title: qsTr("File manager Send to")
                            description: qsTr("In the right-click menu")
                            Toggle {
                                label: qsTr("File manager Send to")
                                checked: Preferences.sendToMenu
                                onToggled: (on) => Preferences.sendToMenu = on
                            }
                        }
                        SettingRow {
                            id: pcInPocketRow
                            width: parent.width
                            iconPath: Icons.folder
                            title: qsTr("Your PC in your pocket")
                            description: qsTr("Downloads, Documents, Desktop, Pictures")
                            Toggle {
                                label: qsTr("Your PC in your pocket")
                                checked: Preferences.sharePcStorage
                                onToggled: (on) => Preferences.sharePcStorage = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.wifi
                            title: qsTr("LocalSend on local network")
                            description: qsTr("Nearby devices, no pairing")
                            Toggle {
                                label: qsTr("LocalSend on local network")
                                checked: AppController.localsendEnabled
                                onToggled: (on) => AppController.toggleLocalsend(on)
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.openExternal
                            title: qsTr("Open files shared from the phone")
                            description: qsTr("As soon as they arrive")
                            Toggle {
                                label: qsTr("Open files shared from the phone")
                                checked: Preferences.openSharedFiles
                                onToggled: (on) => Preferences.openSharedFiles = on
                            }
                        }

                        SettingRow {
                            width: parent.width
                            stackBelow: true
                            iconPath: Icons.mic
                            title: qsTr("Voice recordings")
                            description: Preferences.recordingsFolder
                            Row {
                                spacing: 8
                                Segmented {
                                    anchors.verticalCenter: parent.verticalCenter
                                    options: [
                                        { value: "m4a", label: "M4A" },
                                        { value: "mp3", label: "MP3" },
                                        { value: "wav", label: "WAV" },
                                        { value: "flac", label: "FLAC" }
                                    ]
                                    value: Preferences.recordingsFormat
                                    onPicked: (value) => Preferences.recordingsFormat = value
                                }
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: !Preferences.recordingsFolderIsDefault
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Reset")
                                    onClicked: Preferences.resetRecordingsFolder()
                                }
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    variant: "tonal"
                                    size: "sm"
                                    text: qsTr("Choose folder")
                                    onClicked: recordingsFolderPicker.open()
                                }
                            }
                        }
                    }
                }

                Group {
                    width: parent.width
                    SettingRow {
                        width: parent.width
                        iconPath: Icons.wifi
                        title: qsTr("Sync on Wi-Fi only")
                        description: qsTr("Folder sync and photo backup")
                        Toggle {
                            label: qsTr("Sync on Wi-Fi only")
                            checked: Preferences.syncWifiOnly
                            onToggled: (on) => Preferences.syncWifiOnly = on
                        }
                    }
                }

                // Photo backup: every album, by year and month, never deleting.
                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.photo
                            title: qsTr("Photo backup")
                            description: qsTr("Every album, by year and month · never deletes")
                        }
                        Repeater {
                            // By id: the status changes with every file copied, and rebuilding
                            // the rows would drop a click on a switch.
                            model: page.backupPhoneIds ? page.backupPhoneIds.split(",") : []
                            delegate: Column {
                                id: backupDelegate
                                required property string modelData
                                readonly property var phone: page.backupPhones.find((p) => p.id === modelData) || ({})
                                width: parent.width
                                SettingRow {
                                    width: parent.width
                                    iconPath: Icons.phone
                                    title: backupDelegate.phone.name
                                    description: backupDelegate.phone.status
                                    Toggle {
                                        label: qsTr("Back up photos from %1").arg(backupDelegate.phone.name)
                                        checked: Boolean(backupDelegate.phone.enabled)
                                        onToggled: (on) => PhotoBackup.setEnabled(backupDelegate.phone.id, on)
                                    }
                                }
                                SettingRow {
                                    width: parent.width
                                    visible: Boolean(backupDelegate.phone.enabled)
                                    iconPath: Icons.folder
                                    title: backupDelegate.phone.folder
                                    description: qsTr("Backup folder")
                                    Row {
                                        spacing: 8
                                        Button {
                                            anchors.verticalCenter: parent.verticalCenter
                                            variant: "text"
                                            size: "sm"
                                            text: qsTr("Open")
                                            onClicked: PhotoBackup.openFolder(backupDelegate.phone.id)
                                        }
                                        Button {
                                            anchors.verticalCenter: parent.verticalCenter
                                            variant: "tonal"
                                            size: "sm"
                                            text: qsTr("Choose folder")
                                            onClicked: {
                                                photoBackupFolderPicker.deviceId = backupDelegate.phone.id
                                                photoBackupFolderPicker.open()
                                            }
                                        }
                                    }
                                }
                                SettingRow {
                                    width: parent.width
                                    visible: Boolean(backupDelegate.phone.enabled)
                                    iconPath: Icons.video
                                    title: qsTr("Include videos")
                                    Row {
                                        spacing: 8
                                        Button {
                                            anchors.verticalCenter: parent.verticalCenter
                                            variant: "text"
                                            size: "sm"
                                            iconPath: Icons.refresh
                                            text: qsTr("Back up now")
                                            enabled: Boolean(backupDelegate.phone.online)
                                            onClicked: PhotoBackup.backUpNow(backupDelegate.phone.id)
                                        }
                                        Toggle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            label: qsTr("Include videos")
                                            checked: Boolean(backupDelegate.phone.videos)
                                            onToggled: (on) => PhotoBackup.setVideos(backupDelegate.phone.id, on)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.folder
                            title: qsTr("Synced folders")
                            Row {
                                spacing: 8
                                Button {
                                    variant: "text"
                                    size: "sm"
                                    iconPath: Icons.refresh
                                    text: qsTr("Sync now")
                                    onClicked: Preferences.syncFoldersNow()
                                }
                                Button {
                                    variant: "tonal"
                                    size: "sm"
                                    iconPath: Icons.plus
                                    text: qsTr("Add folder")
                                    // For the phone selected in the app: a rule needs one.
                                    readonly property string phoneId: DeviceList.count > 0
                                        ? DeviceList.deviceIdAt(Math.min(Math.max(0, AppController.currentDevice), DeviceList.count - 1)) || ""
                                        : ""
                                    enabled: phoneId.length > 0
                                    onClicked: folderSyncSheet.openFor(phoneId)
                                }
                            }
                        }
                        Repeater {
                            model: { try { return JSON.parse(Preferences.folderSyncRulesJson || "[]") } catch (e) { return [] } }
                            delegate: Column {
                                id: syncRuleDelegate
                                required property var modelData
                                width: parent.width
                                SettingRow {
                                    width: parent.width
                                    iconPath: Icons.folder
                                    title: syncRuleDelegate.modelData.phoneFolder + (syncRuleDelegate.modelData.mode === "two_way" ? " ⇄ " : syncRuleDelegate.modelData.mode === "one_way_pc_to_phone" ? " ← " : " → ") + syncRuleDelegate.modelData.pcFolder
                                    description: syncRuleDelegate.modelData.mode === "two_way" && syncRuleDelegate.modelData.removeDeleted ? qsTr("Two-way sync · conflict copies kept · deletions go across")
                                        : syncRuleDelegate.modelData.mode === "two_way" ? qsTr("Two-way sync · conflict copies kept")
                                        : syncRuleDelegate.modelData.mode === "one_way_pc_to_phone" ? qsTr("One-way · PC to phone")
                                        : syncRuleDelegate.modelData.removeDeleted ? qsTr("One-way · phone to PC · removes files deleted on the phone")
                                        : qsTr("One-way · phone to PC")
                                    Row {
                                        spacing: 8
                                        Button {
                                            anchors.verticalCenter: parent.verticalCenter
                                            variant: "text"
                                            size: "sm"
                                            iconPath: Icons.trash
                                            text: qsTr("Remove")
                                            onClicked: Preferences.removeFolderSyncRule(syncRuleDelegate.modelData.id)
                                        }
                                        Toggle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            label: syncRuleDelegate.modelData.phoneFolder
                                            checked: Boolean(syncRuleDelegate.modelData.enabled)
                                            onToggled: (on) => Preferences.setFolderSyncRuleEnabled(syncRuleDelegate.modelData.id, on)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "shortcuts"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width

                        SettingRow {
                            id: paletteShortcutRow
                            width: parent.width
                            iconPath: Icons.search
                            title: qsTr("Command palette")
                            Row {
                                spacing: 6
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Preferences.commandPaletteHotkey !== "Ctrl+Alt+Space"
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Reset")
                                    onClicked: {
                                        paletteHotkeyInput.text = "Ctrl+Alt+Space"
                                        Preferences.commandPaletteHotkey = "Ctrl+Alt+Space"
                                    }
                                }
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 124
                                    height: 28
                                    radius: Theme.radiusSm
                                    color: Theme.raised
                                    border.width: paletteHotkeyInput.activeFocus ? 1 : 0
                                    border.color: paletteHotkeyInput.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                    TextInput {
                                        id: paletteHotkeyInput
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        verticalAlignment: TextInput.AlignVCenter
                                        horizontalAlignment: TextInput.AlignHCenter
                                        font.family: Theme.fontUi
                                        font.pixelSize: 12
                                        color: Theme.surfaceContent
                                        selectionColor: Theme.selection
                                        selectedTextColor: Theme.surfaceContent
                                        clip: true
                                        text: Preferences.commandPaletteHotkey
                                        Keys.onPressed: (event) => {
                                            const combo = page.formatKeyCombo(event)
                                            if (combo.length > 0) {
                                                text = combo
                                                Preferences.commandPaletteHotkey = combo
                                                event.accepted = true
                                            }
                                        }
                                        onEditingFinished: Preferences.commandPaletteHotkey = text.trim()
                                        Accessible.name: qsTr("Command palette shortcut")
                                    }
                                }
                            }
                        }
                        SettingRow {
                            id: shelfShortcutRow
                            width: parent.width
                            iconPath: Icons.folder
                            title: qsTr("File shelf")
                            Row {
                                spacing: 6
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Preferences.shelfHotkey !== "Ctrl+Alt+S"
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Reset")
                                    onClicked: {
                                        shelfHotkeyInput.text = "Ctrl+Alt+S"
                                        Preferences.shelfHotkey = "Ctrl+Alt+S"
                                    }
                                }
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 124
                                    height: 28
                                    radius: Theme.radiusSm
                                    color: Theme.raised
                                    border.width: shelfHotkeyInput.activeFocus ? 1 : 0
                                    border.color: shelfHotkeyInput.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                    TextInput {
                                        id: shelfHotkeyInput
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        verticalAlignment: TextInput.AlignVCenter
                                        horizontalAlignment: TextInput.AlignHCenter
                                        font.family: Theme.fontUi
                                        font.pixelSize: 12
                                        color: Theme.surfaceContent
                                        selectionColor: Theme.selection
                                        selectedTextColor: Theme.surfaceContent
                                        clip: true
                                        text: Preferences.shelfHotkey
                                        Keys.onPressed: (event) => {
                                            const combo = page.formatKeyCombo(event)
                                            if (combo.length > 0) {
                                                text = combo
                                                Preferences.shelfHotkey = combo
                                                event.accepted = true
                                            }
                                        }
                                        onEditingFinished: Preferences.shelfHotkey = text.trim()
                                        Accessible.name: qsTr("Shelf shortcut")
                                    }
                                }
                            }
                        }

                        SettingRow {
                            id: photoShortcutRow
                            width: parent.width
                            iconPath: Icons.camera
                            title: qsTr("Take photo")
                            Row {
                                spacing: 6
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Preferences.continuityPhotoHotkey !== "Ctrl+Alt+C"
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Reset")
                                    onClicked: {
                                        photoHotkeyInput.text = "Ctrl+Alt+C"
                                        Preferences.continuityPhotoHotkey = "Ctrl+Alt+C"
                                    }
                                }
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 124
                                    height: 28
                                    radius: Theme.radiusSm
                                    color: Theme.raised
                                    border.width: photoHotkeyInput.activeFocus ? 1 : 0
                                    border.color: photoHotkeyInput.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                    TextInput {
                                        id: photoHotkeyInput
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        verticalAlignment: TextInput.AlignVCenter
                                        horizontalAlignment: TextInput.AlignHCenter
                                        font.family: Theme.fontUi
                                        font.pixelSize: 12
                                        color: Theme.surfaceContent
                                        selectionColor: Theme.selection
                                        selectedTextColor: Theme.surfaceContent
                                        clip: true
                                        text: Preferences.continuityPhotoHotkey
                                        Keys.onPressed: (event) => {
                                            const combo = page.formatKeyCombo(event)
                                            if (combo.length > 0) {
                                                text = combo
                                                Preferences.continuityPhotoHotkey = combo
                                                event.accepted = true
                                            }
                                        }
                                        onEditingFinished: Preferences.continuityPhotoHotkey = text.trim()
                                        Accessible.name: qsTr("Take photo shortcut")
                                    }
                                }
                            }
                        }
                        SettingRow {
                            id: scanShortcutRow
                            width: parent.width
                            iconPath: Icons.scan
                            title: qsTr("Scan document")
                            Row {
                                spacing: 6
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Preferences.continuityScanHotkey !== "Ctrl+Alt+D"
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Reset")
                                    onClicked: {
                                        scanHotkeyInput.text = "Ctrl+Alt+D"
                                        Preferences.continuityScanHotkey = "Ctrl+Alt+D"
                                    }
                                }
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 124
                                    height: 28
                                    radius: Theme.radiusSm
                                    color: Theme.raised
                                    border.width: scanHotkeyInput.activeFocus ? 1 : 0
                                    border.color: scanHotkeyInput.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                    TextInput {
                                        id: scanHotkeyInput
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        verticalAlignment: TextInput.AlignVCenter
                                        horizontalAlignment: TextInput.AlignHCenter
                                        font.family: Theme.fontUi
                                        font.pixelSize: 12
                                        color: Theme.surfaceContent
                                        selectionColor: Theme.selection
                                        selectedTextColor: Theme.surfaceContent
                                        clip: true
                                        text: Preferences.continuityScanHotkey
                                        Keys.onPressed: (event) => {
                                            const combo = page.formatKeyCombo(event)
                                            if (combo.length > 0) {
                                                text = combo
                                                Preferences.continuityScanHotkey = combo
                                                event.accepted = true
                                            }
                                        }
                                        onEditingFinished: Preferences.continuityScanHotkey = text.trim()
                                        Accessible.name: qsTr("Scan document shortcut")
                                    }
                                }
                            }
                        }
                    }
                }

                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.bolt
                            title: qsTr("Trigger-action rules")
                            Row {
                                spacing: 8
                                Button {
                                    variant: "tonal"
                                    size: "sm"
                                    iconPath: Icons.plus
                                    text: qsTr("Lock PC on disconnect")
                                    onClicked: Preferences.addAutomationRule("phone_disconnected", "lock_pc", "")
                                }
                                Button {
                                    variant: "tonal"
                                    size: "sm"
                                    iconPath: Icons.plus
                                    text: qsTr("Pause media on ring")
                                    onClicked: Preferences.addAutomationRule("call_ringing", "pause_pc_media", "")
                                }
                            }
                        }
                        Repeater {
                            model: { try { return JSON.parse(Preferences.automationRulesJson || "[]") } catch (e) { return [] } }
                            delegate: Column {
                                id: autoRuleDelegate
                                required property var modelData
                                width: parent.width
                                SettingRow {
                                    width: parent.width
                                    iconPath: Icons.bolt
                                    title: page.formatTrigger(autoRuleDelegate.modelData.trigger) + " → " + page.formatAction(autoRuleDelegate.modelData.actionKind)
                                    description: autoRuleDelegate.modelData.actionArg && autoRuleDelegate.modelData.actionArg.length > 0
                                        ? autoRuleDelegate.modelData.actionArg
                                        : ""
                                    Row {
                                        spacing: 8
                                        Button {
                                            anchors.verticalCenter: parent.verticalCenter
                                            variant: "text"
                                            size: "sm"
                                            iconPath: Icons.trash
                                            text: qsTr("Remove")
                                            onClicked: Preferences.removeAutomationRule(autoRuleDelegate.modelData.id)
                                        }
                                        Toggle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            label: autoRuleDelegate.modelData.trigger
                                            checked: Boolean(autoRuleDelegate.modelData.enabled)
                                            onToggled: (on) => Preferences.setAutomationRuleEnabled(autoRuleDelegate.modelData.id, on)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // OBS Studio, for Deck tiles that run "obs:" commands.
                Group {
                    id: obsCard
                    width: parent.width
                    property bool hasPassword: Preferences.obsHasPassword()
                    Column {
                        width: parent.width
                        spacing: 12
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.terminal
                            title: qsTr("OBS Studio")
                            description: qsTr("For obs: Deck tiles · turn on OBS's WebSocket server")
                        }
                        Row {
                            spacing: 10
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                role: "bodySmall"
                                muted: true
                                text: qsTr("Port")
                            }
                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 76
                                height: 28
                                radius: Theme.radiusSm
                                color: Theme.raised
                                border.width: obsPortEdit.activeFocus ? 1 : 0
                                border.color: obsPortEdit.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                TextInput {
                                    id: obsPortEdit
                                    anchors.fill: parent
                                    anchors.leftMargin: 8
                                    anchors.rightMargin: 8
                                    verticalAlignment: TextInput.AlignVCenter
                                    horizontalAlignment: TextInput.AlignHCenter
                                    color: Theme.surfaceContent
                                    selectionColor: Theme.selection
                                    selectedTextColor: Theme.surfaceContent
                                    font.family: Theme.fontMono
                                    font.pixelSize: 12
                                    text: String(Preferences.obsPort())
                                    validator: IntValidator { bottom: 1024; top: 65535 }
                                    Accessible.name: qsTr("OBS WebSocket port")
                                }
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                role: "bodySmall"
                                muted: true
                                text: qsTr("Password")
                            }
                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 180
                                height: 28
                                radius: Theme.radiusSm
                                color: Theme.raised
                                border.width: obsPasswordEdit.activeFocus ? 1 : 0
                                border.color: obsPasswordEdit.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                TextInput {
                                    id: obsPasswordEdit
                                    anchors.fill: parent
                                    anchors.leftMargin: 8
                                    anchors.rightMargin: 8
                                    verticalAlignment: TextInput.AlignVCenter
                                    color: Theme.surfaceContent
                                    selectionColor: Theme.selection
                                    selectedTextColor: Theme.surfaceContent
                                    font.pixelSize: 12
                                    echoMode: TextInput.Password
                                    passwordCharacter: "\u2022"
                                    Accessible.name: qsTr("OBS WebSocket password")
                                    Txt {
                                        anchors.verticalCenter: parent.verticalCenter
                                        visible: obsPasswordEdit.text.length === 0 && !obsPasswordEdit.activeFocus
                                        role: "bodySmall"
                                        muted: true
                                        text: obsCard.hasPassword ? qsTr("Saved") : qsTr("None")
                                    }
                                }
                            }
                        }
                        Row {
                            spacing: 8
                            Button {
                                size: "sm"
                                text: qsTr("Save")
                                onClicked: {
                                    Preferences.saveObs(parseInt(obsPortEdit.text) || Preferences.obsPort(),
                                                        obsPasswordEdit.text, obsPasswordEdit.text.length > 0)
                                    obsPasswordEdit.text = ""
                                    obsCard.hasPassword = Preferences.obsHasPassword()
                                }
                            }
                            Button {
                                size: "sm"
                                variant: "tonal"
                                text: qsTr("Test connection")
                                onClicked: Preferences.testObs()
                            }
                            Button {
                                size: "sm"
                                variant: "tonal"
                                visible: obsCard.hasPassword
                                text: qsTr("Remove password")
                                onClicked: {
                                    Preferences.saveObs(Preferences.obsPort(), "", true)
                                    obsCard.hasPassword = Preferences.obsHasPassword()
                                }
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "storage"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.lock
                            title: qsTr("Sync copied passwords")
                            description: qsTr("Never kept in history")
                            Toggle {
                                label: qsTr("Sync copied passwords")
                                checked: Preferences.syncPasswords
                                onToggled: (on) => Preferences.syncPasswords = on
                            }
                        }

                        SettingRow {
                            id: clipHistRow
                            width: parent.width
                            iconPath: Icons.clipboard
                            title: qsTr("Clipboard history")
                            description: {
                                const count = Number(page.retentionSummary.clipboardItems || 0)
                                const size = page.formatRetentionBytes(page.retentionSummary.clipboardBytes)
                                return qsTr("%1 clips · %2 · kept 7 days").arg(count).arg(size)
                            }
                            Row {
                                spacing: 8
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Number(page.retentionSummary.clipboardItems || 0) > 0
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Clear")
                                    onClicked: clearOneSheet.ask(
                                        qsTr("Clear clipboard history?"),
                                        qsTr("Deletes the clips kept on this PC. What's on the clipboard now stays."),
                                        () => {
                                            AppController.clearClipboardHistory()
                                            AppController.refreshDataRetention()
                                        })
                                }
                                Toggle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    label: qsTr("Clipboard history")
                                    checked: Preferences.clipboardHistory
                                    onToggled: (on) => {
                                        Preferences.clipboardHistory = on
                                        AppController.refreshDataRetention()
                                    }
                                }
                            }
                        }
                        SettingRow {
                            id: notifHistRow
                            width: parent.width
                            iconPath: Icons.bell
                            title: qsTr("Notification history")
                            description: {
                                const count = Number(page.retentionSummary.notificationHistory || 0)
                                const size = page.formatRetentionBytes(page.retentionSummary.notificationImageBytes)
                                return qsTr("%1 alerts · %2 · kept 24h").arg(count).arg(size)
                            }
                            Row {
                                spacing: 8
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Number(page.retentionSummary.notificationHistory || 0) > 0
                                             || Number(page.retentionSummary.notificationImageBytes || 0) > 0
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Clear")
                                    onClicked: clearOneSheet.ask(
                                        qsTr("Clear notification history?"),
                                        qsTr("Deletes the notifications kept after they went away. Ones still showing stay."),
                                        () => AppController.clearNotificationHistory())
                                }
                                Toggle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    label: qsTr("Notification history")
                                    checked: NotificationList.historyEnabled
                                    onToggled: (on) => {
                                        NotificationList.setHistoryEnabled(on)
                                        AppController.refreshDataRetention()
                                    }
                                }
                            }
                        }

                        SettingRow {
                            width: parent.width
                            stackBelow: true
                            iconPath: Icons.history
                            title: qsTr("Cross-device timeline")
                            description: {
                                const count = Number(page.retentionSummary.timelineItems || 0)
                                return count === 1
                                    ? qsTr("1 item stored (max 5,000).")
                                    : qsTr("%1 items stored (max 5,000).").arg(count)
                            }
                            Row {
                                spacing: 8
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: Number(page.retentionSummary.timelineItems || 0) > 0
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Clear")
                                    onClicked: clearOneSheet.ask(
                                        qsTr("Clear the Timeline?"),
                                        qsTr("Deletes the Timeline's entries on this PC. Files stay where they were saved."),
                                        () => {
                                            TimelineModel.clearAll()
                                            AppController.refreshDataRetention()
                                        })
                                }
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Open")
                                    onClicked: AppController.currentPage = "timeline"
                                }
                                Segmented {
                                    anchors.verticalCenter: parent.verticalCenter
                                    options: [
                                        { value: "30", label: qsTr("30d") },
                                        { value: "90", label: qsTr("90d") },
                                        { value: "365", label: qsTr("1y") },
                                        { value: "0", label: qsTr("All") }
                                    ]
                                    value: String(Preferences.timelineRetentionDays)
                                    onPicked: (value) => {
                                        Preferences.timelineRetentionDays = Number(value)
                                        AppController.refreshDataRetention()
                                    }
                                }
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.messages
                            title: qsTr("Cached messages")
                            description: {
                                const chatThreads = Number(page.retentionSummary.chatThreads || 0)
                                const smsThreads = Number(page.retentionSummary.cachedSmsThreads || 0)
                                const convs = chatThreads + smsThreads
                                const totalMsgs = Number(page.retentionSummary.chatMessages || 0)
                                                  + Number(page.retentionSummary.cachedSmsMessages || 0)
                                const size = page.formatRetentionBytes(page.retentionSummary.messageAttachmentBytes)
                                const convPart = convs === 1 ? qsTr("1 chat") : qsTr("%1 chats").arg(convs)
                                const msgPart = totalMsgs === 1 ? qsTr("1 message") : qsTr("%1 messages").arg(totalMsgs)
                                return qsTr("%1 · %2 · %3 · kept 90 days")
                                    .arg(convPart)
                                    .arg(msgPart)
                                    .arg(size)
                            }
                            Button {
                                variant: "text"
                                size: "sm"
                                visible: Number(page.retentionSummary.chatThreads || 0) > 0
                                         || Number(page.retentionSummary.cachedSmsThreads || 0) > 0
                                         || Number(page.retentionSummary.chatMessages || 0) > 0
                                         || Number(page.retentionSummary.cachedSmsMessages || 0) > 0
                                         || Number(page.retentionSummary.messageAttachmentBytes || 0) > 0
                                text: qsTr("Clear")
                                onClicked: AppController.clearMessageCache()
                            }
                        }
                        SettingRow {
                            id: photoCacheRow
                            width: parent.width
                            iconPath: Icons.photo
                            title: qsTr("Photo thumbnails")
                            description: {
                                const files = Number(page.retentionSummary.photoThumbFiles || 0)
                                const size = page.formatRetentionBytes(page.retentionSummary.photoThumbBytes)
                                return qsTr("%1 cached · %2 · kept 30 days").arg(files).arg(size)
                            }
                            Button {
                                variant: "text"
                                size: "sm"
                                visible: Number(page.retentionSummary.photoThumbFiles || 0) > 0
                                text: qsTr("Clear")
                                onClicked: AppController.clearPhotoThumbnails()
                            }
                        }
                        SettingRow {
                            id: transferHistRow
                            width: parent.width
                            iconPath: Icons.send
                            title: qsTr("Transfer history")
                            description: {
                                const records = Number(page.retentionSummary.receivedFileRecords || 0)
                                return qsTr("%1 records · kept with timeline").arg(records)
                            }
                            Button {
                                variant: "text"
                                size: "sm"
                                visible: Number(page.retentionSummary.receivedFileRecords || 0) > 0
                                text: qsTr("Clear")
                                onClicked: clearOneSheet.ask(
                                    qsTr("Clear transfer history?"),
                                    qsTr("Deletes the list of past transfers. The files stay where they were saved."),
                                    () => AppController.clearReceivedFileHistory())
                            }
                        }

                        SettingRow {
                            width: parent.width
                            iconPath: Icons.trash
                            title: qsTr("Clear all history and caches")
                            description: qsTr("Keeps phones and settings")
                            Button {
                                variant: "outline"
                                size: "sm"
                                iconPath: Icons.trash
                                text: qsTr("Clear everything…")
                                onClicked: clearEverythingSheet.open()
                            }
                        }
                    }
                }
            }

            Column {
                visible: page.currentSection === "connection"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.doctor
                            title: qsTr("Connection Doctor")
                            description: qsTr("Firewall, network and VPN")
                            Button {
                                variant: "tonal"
                                size: "sm"
                                text: qsTr("Check")
                                onClicked: AppController.runDoctor()
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.signal
                            title: qsTr("Connect over mobile data")
                            description: qsTr("Away from Wi-Fi")
                            Toggle {
                                checked: Preferences.awayMode
                                onToggled: function(v) { Preferences.awayMode = v }
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.power
                            title: qsTr("Wake from your phone")
                            description: {
                                const adapter = AppController.wakeAdapter
                                const state = AppController.wakeState
                                if (state === "none" || adapter.length === 0)
                                    return qsTr("No active network adapter found.")
                                if (state === "enabled")
                                    return qsTr("%1 · Wake on Magic Packet ready.").arg(adapter)
                                return qsTr("%1 · Wake on Magic Packet is off (NetworkManager or ethtool)").arg(adapter)
                            }
                            Row {
                                spacing: 6
                                StatusDot {
                                    anchors.verticalCenter: parent.verticalCenter
                                    online: AppController.wakeState === "enabled"
                                }
                                Txt {
                                    anchors.verticalCenter: parent.verticalCenter
                                    role: "bodySmall"
                                    muted: true
                                    text: AppController.wakeState === "enabled" ? qsTr("Ready")
                                        : AppController.wakeState === "disabled"
                                          ? (qsTr("Off in the adapter's settings"))
                                        : AppController.wakeState === "unknown" ? qsTr("Check settings")
                                        : qsTr("No adapter")
                                }
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.terminal
                            title: qsTr("Local automation API")
                            description: Preferences.localApiEnabled
                                ? qsTr("Listening on 127.0.0.1:%1 · Token in %2").arg(Preferences.localApiPort).arg(Preferences.localApiTokenPath)
                                : qsTr("Local HTTP/SSE server (127.0.0.1:%1) for scripts and tools.").arg(Preferences.localApiPort)
                            Row {
                                spacing: 10
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 76
                                    height: 28
                                    radius: Theme.radiusSm
                                    color: Theme.raised
                                    border.width: localApiPortEdit.activeFocus ? 1 : 0
                                    border.color: localApiPortEdit.activeFocus ? Theme.focusRing : Theme.outlineVariant
                                    TextInput {
                                        id: localApiPortEdit
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        verticalAlignment: TextInput.AlignVCenter
                                        horizontalAlignment: TextInput.AlignHCenter
                                        color: Theme.surfaceContent
                                        selectionColor: Theme.selection
                                        selectedTextColor: Theme.surfaceContent
                                        font.family: Theme.fontMono
                                        font.pixelSize: 12
                                        text: String(Preferences.localApiPort)
                                        validator: IntValidator { bottom: 1024; top: 65535 }
                                        onEditingFinished: {
                                            const parsed = parseInt(text, 10)
                                            if (!isNaN(parsed) && parsed >= 1024 && parsed <= 65535)
                                                Preferences.localApiPort = parsed
                                            else
                                                text = String(Preferences.localApiPort)
                                        }
                                    }
                                }
                                Toggle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    label: qsTr("Local automation API")
                                    checked: Preferences.localApiEnabled
                                    onToggled: (on) => Preferences.localApiEnabled = on
                                }
                            }
                        }
                    }
                }
            }

            // ================================================================
            // ABOUT
            // ================================================================
            Column {
                visible: page.currentSection === "about"
                width: visible ? parent.width : 0
                spacing: 14

                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.desktop
                            title: qsTr("G7 Link %1").arg(AppController.version)
                            description: AppController.updateStatus.length > 0
                                         ? AppController.updateStatus
                                         : qsTr("This PC: %1").arg(AppController.deviceName)
                            Row {
                                spacing: 8
                                // The download's progress, or a spinner while its size isn't known.
                                Rectangle {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: AppController.updateBusy && AppController.updateProgress >= 0
                                    width: 120
                                    height: 4
                                    radius: 2
                                    color: Theme.raised
                                    Accessible.role: Accessible.ProgressBar
                                    Accessible.name: qsTr("Update download")
                                    Rectangle {
                                        height: parent.height
                                        radius: parent.radius
                                        width: parent.width * Math.max(0, Math.min(1, AppController.updateProgress))
                                        color: Theme.accent
                                        Behavior on width { enabled: !Theme.reduceMotion; NumberAnimation { duration: 180 } }
                                    }
                                }
                                Spinner {
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: AppController.updateBusy && AppController.updateProgress < 0
                                }
                                Button {
                                    visible: AppController.canUpdate
                                    anchors.verticalCenter: parent.verticalCenter
                                    size: "sm"
                                    variant: AppController.updateVersion.length > 0 ? "fill" : "tonal"
                                    iconPath: Icons.refresh
                                    enabled: !AppController.updateBusy
                                    text: AppController.updateVersion.length > 0
                                          ? qsTr("Update to %1").arg(AppController.updateVersion)
                                          : qsTr("Check for updates")
                                    onClicked: AppController.updateVersion.length > 0
                                               ? AppController.installUpdate() : AppController.checkForUpdates()
                                }
                                Button {
                                    anchors.verticalCenter: parent.verticalCenter
                                    variant: "outline"
                                    size: "sm"
                                    iconPath: Icons.folder
                                    text: qsTr("Open logs")
                                    onClicked: Qt.openUrlExternally(AppController.logsUrl())
                                }
                            }
                        }
                        SettingRow {
                            width: parent.width
                            visible: AppController.canUpdate
                            iconPath: Icons.refresh
                            title: qsTr("Check for updates")
                            description: Preferences.updateChannel === "nightly"
                                         ? qsTr("Every couple of hours")
                                         : qsTr("Twice a day")
                            Toggle {
                                label: qsTr("Check for updates")
                                checked: Preferences.autoUpdate
                                onToggled: (on) => Preferences.autoUpdate = on
                            }
                        }
                        SettingRow {
                            width: parent.width
                            visible: AppController.canUpdate
                            iconPath: Icons.channel
                            title: qsTr("Update channel")
                            description: Preferences.updateChannel === "nightly"
                                         ? qsTr("Newest build, sometimes new bugs")
                                         : qsTr("Tested releases")
                            Segmented {
                                options: [
                                    { value: "stable", label: qsTr("Stable") },
                                    { value: "nightly", label: qsTr("Nightly") }
                                ]
                                value: Preferences.updateChannel
                                onPicked: (value) => Preferences.updateChannel = value
                            }
                        }
                    }
                }

                Group {
                    width: parent.width
                    Column {
                        width: parent.width
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.compass
                            title: qsTr("Welcome screen")
                            description: qsTr("This PC's checks and pairing")
                            Button {
                                variant: "tonal"
                                size: "sm"
                                text: qsTr("Open")
                                onClicked: page.restartWalkthroughRequested()
                            }
                        }
                        SettingRow {
                            width: parent.width
                            iconPath: Icons.book
                            title: qsTr("Features guide")
                            Button {
                                variant: "tonal"
                                size: "sm"
                                text: qsTr("Open")
                                onClicked: AppController.currentPage = "guide"
                            }
                        }
                    }
                }

                // The phone app, for a phone that doesn't have it yet.
                Group {
                    width: parent.width
                    title: qsTr("Android app")
                    SettingRow {
                        width: parent.width
                        iconPath: Icons.download
                        title: qsTr("Download")
                        description: qsTr("Android 8.0 or later")
                        Row {
                            spacing: 4
                            TextAction {
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("Open release page")
                                onClicked: Qt.openUrlExternally(page.releaseUrl)
                            }
                            TextAction {
                                id: copyLink
                                property bool copied: false
                                anchors.verticalCenter: parent.verticalCenter
                                text: copied ? qsTr("Copied") : qsTr("Copy link")
                                onClicked: {
                                    AppController.copyText(page.releaseUrl)
                                    copied = true
                                    copyLinkReset.restart()
                                }
                                Timer {
                                    id: copyLinkReset
                                    interval: 2000
                                    onTriggered: copyLink.copied = false
                                }
                            }
                        }
                    }
                    SettingRow {
                        width: parent.width
                        iconPath: Icons.usb
                        title: qsTr("Install over USB")
                        description: page.adbCommand
                        TextAction {
                            id: copyAdb
                            property bool copied: false
                            text: copied ? qsTr("Copied") : qsTr("Copy command")
                            onClicked: {
                                AppController.copyText(page.adbCommand)
                                copied = true
                                copyAdbReset.restart()
                            }
                            Timer {
                                id: copyAdbReset
                                interval: 2000
                                onTriggered: copyAdb.copied = false
                            }
                        }
                    }
                }
            }
        }
    }

    Connections {
        target: AppController
        function onHighlightTargetChanged() {
            const tgt = AppController.highlightTarget
            if (tgt.startsWith("settings:")) {
                const parts = tgt.split(":")
                if (parts.length > 1) {
                    let sec = parts[1]
                    if (sec === "camera" || sec === "audio") sec = "camera_audio"
                    else if (sec === "clipboard" || sec === "files" || sec === "sync") sec = "files_sync"
                    else if (sec === "surfaces" || sec === "automation" || sec === "hotkeys") sec = "shortcuts"
                    else if (sec === "privacy" || sec === "history") sec = "storage"
                    else if (sec === "phone" || sec === "permissions") sec = "devices"
                    else if (sec === "updates" || sec === "version") sec = "about"
                    page.currentSection = sec
                }
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 10
        radius: Theme.radiusXl
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: opacity > 0
        opacity: AppController.highlightTarget.startsWith("settings:") ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }

    // Synced folders: which phone folder, where on this PC, and which way.
    Sheet {
        id: folderSyncSheet
        property string phoneId
        property string phoneFolder
        property string pcFolder
        property string mode: "one_way_phone_to_pc"
        property bool removeDeleted: false
        // The PC folder follows the phone folder until the user picks one.
        property bool pcChosen: false
        readonly property var common: ["DCIM/Camera", "Pictures/Screenshots", "Download", "Documents", "Music", "Movies", "Recordings"]
        readonly property string tidyPhoneFolder: phoneFolder.split(/[\/\\]/).map(p => p.trim()).filter(p => p.length > 0 && p !== ".").join("/")
        readonly property bool phoneFolderOk: tidyPhoneFolder.length > 0 && tidyPhoneFolder.split("/").indexOf("..") < 0
        cardWidth: 480

        function openFor(id) {
            phoneId = id
            mode = "one_way_phone_to_pc"
            removeDeleted = false
            pcChosen = false
            choosePhoneFolder("DCIM/Camera")
            open()
        }
        function choosePhoneFolder(folder) {
            phoneFolder = folder
            if (phoneFolderInput.text !== folder)
                phoneFolderInput.text = folder
            if (!pcChosen)
                pcFolder = Preferences.suggestedSyncFolder(folder)
        }

        Column {
            width: parent.width
            spacing: 12
            Txt { width: parent.width; text: qsTr("Sync a folder"); role: "headline"; wrapMode: Text.WordWrap }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: qsTr("The phone asks once to let this PC in. G7 Link there also needs All files access, or this folder under Selected folders only.")
            }

            Txt { text: qsTr("Folder on the phone"); role: "label" }
            Flow {
                width: parent.width
                spacing: 6
                Repeater {
                    model: folderSyncSheet.common
                    delegate: Chip {
                        required property string modelData
                        text: modelData
                        interactive: true
                        selected: folderSyncSheet.tidyPhoneFolder === modelData
                        onClicked: folderSyncSheet.choosePhoneFolder(modelData)
                    }
                }
            }
            Rectangle {
                width: parent.width
                height: 40
                radius: Theme.radiusMd
                color: Theme.surfaceContainerHighest
                border.width: phoneFolderInput.activeFocus ? 2 : 1
                border.color: phoneFolderInput.activeFocus ? Theme.focusRing : Theme.outlineVariant
                TextInput {
                    id: phoneFolderInput
                    anchors.fill: parent
                    anchors.margins: 10
                    color: Theme.surfaceContent
                    font.pixelSize: 14
                    selectByMouse: true
                    clip: true
                    Accessible.role: Accessible.EditableText
                    Accessible.name: qsTr("Folder on the phone")
                    onTextEdited: folderSyncSheet.choosePhoneFolder(text)
                }
            }

            Txt { text: qsTr("Folder on this PC"); role: "label" }
            Row {
                width: parent.width
                spacing: 8
                Txt {
                    width: parent.width - choosePcFolder.width - 8
                    anchors.verticalCenter: parent.verticalCenter
                    text: folderSyncSheet.pcFolder
                    elide: Text.ElideMiddle
                    role: "body"
                }
                Button {
                    id: choosePcFolder
                    variant: "text"
                    size: "sm"
                    text: qsTr("Choose…")
                    onClicked: {
                        folderSyncSheet.pcChosen = true
                        folderSyncPcPicker.open()
                    }
                }
            }

            Segmented {
                options: [
                    { value: "one_way_phone_to_pc", label: qsTr("Phone to PC") },
                    { value: "one_way_pc_to_phone", label: qsTr("PC to phone") },
                    { value: "two_way", label: qsTr("Both ways") }
                ]
                value: folderSyncSheet.mode
                onPicked: (v) => folderSyncSheet.mode = v
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: folderSyncSheet.mode === "one_way_pc_to_phone"
                    ? qsTr("New and changed files on the PC are copied to the phone. Nothing on the phone comes to the PC, and nothing is deleted.")
                    : folderSyncSheet.mode === "two_way"
                    ? qsTr("Changes on either side are copied to the other. When a file changed on both, both versions are kept.")
                    : qsTr("New and changed files on the phone are copied to the PC. Nothing on the PC goes to the phone.")
            }

            SettingRow {
                width: parent.width
                visible: folderSyncSheet.mode !== "one_way_pc_to_phone"
                title: folderSyncSheet.mode === "two_way" ? qsTr("Also remove deleted files") : qsTr("Also remove from the PC")
                description: folderSyncSheet.mode === "two_way"
                    ? qsTr("A synced file you delete on one side leaves the other. On the PC it goes to the Recycle Bin. On the phone, photos and videos go to its trash where Android has one, and other files are gone for good.")
                    : qsTr("A synced file you delete on the phone goes to the PC's Recycle Bin.")
                Toggle {
                    label: folderSyncSheet.mode === "two_way" ? qsTr("Also remove deleted files") : qsTr("Also remove from the PC")
                    checked: folderSyncSheet.removeDeleted
                    onToggled: (on) => folderSyncSheet.removeDeleted = on
                }
            }

            Row {
                anchors.right: parent.right
                spacing: 10
                Button { variant: "text"; text: qsTr("Cancel"); onClicked: folderSyncSheet.close() }
                Button {
                    text: qsTr("Add")
                    enabled: folderSyncSheet.phoneFolderOk && folderSyncSheet.pcFolder.length > 0
                    onClicked: {
                        Preferences.addFolderSyncRule(folderSyncSheet.phoneId, folderSyncSheet.tidyPhoneFolder,
                                                      folderSyncSheet.pcFolder, folderSyncSheet.mode,
                                                      folderSyncSheet.removeDeleted)
                        folderSyncSheet.close()
                    }
                }
            }
        }
    }

    Sheet {
        id: hotspotSheet
        property string deviceId
        property var row: null
        property var networks: []
        cardWidth: 420
        Column {
            width: parent.width
            spacing: 12
            Txt { width: parent.width; text: qsTr("Phone's hotspot network"); role: "headline"; wrapMode: Text.WordWrap }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: (hotspotSheet.networks.length > 0
                        ? qsTr("Wi-Fi networks this PC has joined before. Your system keeps their passwords; G7 Link never sees them.")
                        : qsTr("This PC hasn't saved any Wi-Fi networks yet. Join the phone's hotspot once from your system Wi-Fi list first."))
            }
            // A PC can have saved dozens of networks: the list scrolls.
            ListView {
                width: parent.width
                height: Math.min(contentHeight, 320)
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                spacing: 4
                model: hotspotSheet.networks
                delegate: Button {
                    required property var modelData
                    width: ListView.view.width
                    readonly property bool chosen: hotspotSheet.row !== null && hotspotSheet.row.network === modelData
                    variant: chosen ? "tonal" : "text"
                    iconPath: chosen ? Icons.check : Icons.wifi
                    text: modelData
                    onClicked: {
                        AppController.setHotspotNetwork(hotspotSheet.deviceId, modelData)
                        if (hotspotSheet.row !== null)
                            hotspotSheet.row.network = modelData
                        hotspotSheet.close()
                    }
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Don't join")
                    visible: hotspotSheet.row !== null && hotspotSheet.row.network.length > 0
                    onClicked: {
                        AppController.setHotspotNetwork(hotspotSheet.deviceId, "")
                        if (hotspotSheet.row !== null)
                            hotspotSheet.row.network = ""
                        hotspotSheet.close()
                    }
                }
                Button { variant: "text"; text: qsTr("Close"); onClicked: hotspotSheet.close() }
            }
        }
    }

    Sheet {
        id: languageSheet
        cardWidth: 400
        Column {
            width: parent.width
            spacing: 12
            Txt { width: parent.width; text: qsTr("Language"); role: "headline"; wrapMode: Text.WordWrap }
            Repeater {
                model: page.languages
                delegate: Button {
                    required property var modelData
                    width: parent.width
                    variant: modelData.code === Preferences.language ? "tonal" : "text"
                    iconPath: modelData.code === Preferences.language ? Icons.check : Icons.globe
                    text: modelData.name
                    onClicked: {
                        Preferences.language = modelData.code
                        languageSheet.close()
                    }
                }
            }
            Row {
                anchors.right: parent.right
                Button { variant: "text"; text: qsTr("Close"); onClicked: languageSheet.close() }
            }
        }
    }

    Sheet {
        id: micOutputSheet
        cardWidth: 440
        Column {
            width: parent.width
            spacing: 12
            Txt { width: parent.width; text: qsTr("Play the phone's microphone into"); role: "headline"; wrapMode: Text.WordWrap }
            Repeater {
                model: { try { return JSON.parse(PhoneMic.outputs) } catch (e) { return [] } }
                delegate: Button {
                    required property var modelData
                    width: parent.width
                    variant: modelData.id === PhoneMic.output ? "tonal" : "text"
                    iconPath: modelData.id === PhoneMic.output ? Icons.check : Icons.speaker
                    text: modelData.name
                    onClicked: {
                        PhoneMic.chooseOutput(modelData.id)
                        micOutputSheet.close()
                    }
                }
            }
            Row {
                anchors.right: parent.right
                Button { variant: "text"; text: qsTr("Close"); onClicked: micOutputSheet.close() }
            }
        }
    }

    Sheet {
        id: unpairSheet
        property string deviceId
        property string deviceName
        cardWidth: 400
        Column {
            width: parent.width
            spacing: 16
            Txt { width: parent.width; text: qsTr("Unpair %1?").arg(unpairSheet.deviceName); role: "headline"; wrapMode: Text.WordWrap }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: qsTr("Both devices forget each other. You can pair them again any time.")
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button { variant: "text"; text: qsTr("Cancel"); onClicked: unpairSheet.close() }
                Button {
                    text: qsTr("Unpair")
                    onClicked: {
                        AppController.unpair(unpairSheet.deviceId)
                        unpairSheet.close()
                    }
                }
            }
        }
    }

    // Asks before clearing one kind of history: it can't be brought back.
    Sheet {
        id: clearOneSheet
        cardWidth: 420
        property string title: ""
        property string detail: ""
        property var action: null
        function ask(title, detail, action) {
            clearOneSheet.title = title
            clearOneSheet.detail = detail
            clearOneSheet.action = action
            open()
        }
        Column {
            width: parent.width
            spacing: 16
            Txt {
                width: parent.width
                text: clearOneSheet.title
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: clearOneSheet.detail
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button { variant: "text"; text: qsTr("Cancel"); onClicked: clearOneSheet.close() }
                Button {
                    iconPath: Icons.trash
                    text: qsTr("Clear")
                    onClicked: {
                        if (clearOneSheet.action)
                            clearOneSheet.action()
                        clearOneSheet.close()
                    }
                }
            }
        }
    }

    Sheet {
        id: clearEverythingSheet
        cardWidth: 420
        Column {
            width: parent.width
            spacing: 16
            Txt {
                width: parent.width
                text: qsTr("Clear all local history & caches?")
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: qsTr("Deletes clipboard history, Timeline, notification history, cached messages, photo thumbnails and transfer history on this PC. Paired devices, settings and downloaded files are kept.")
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button { variant: "text"; text: qsTr("Cancel"); onClicked: clearEverythingSheet.close() }
                Button {
                    iconPath: Icons.trash
                    text: qsTr("Clear everything")
                    onClicked: {
                        AppController.clearEverything()
                        clearEverythingSheet.close()
                    }
                }
            }
        }
    }
}
