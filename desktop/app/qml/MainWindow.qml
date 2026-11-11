// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import QtQuick.Effects
import QtQml
import QtQml.Models
import app.g7link

// The main window: a header (phone picker and search), a sidebar, the page
// and a footer (the G7 menu, the link and battery, the page's actions).
// Before any device is paired it shows the welcome screen instead.
NativeWindow {
    id: window

    readonly property bool showWalkthrough: AppController.status === "ready"
                                            && (!Preferences.walkthroughCompleted || uiCheckWelcome)
    readonly property bool welcome: AppController.status !== "ready" || showWalkthrough
    property string page: AppController.currentPage.length > 0 ? AppController.currentPage : "home"
    property string pendingSettingsSection: ""
    onPageChanged: if (AppController.currentPage !== page) AppController.currentPage = page
    Connections {
        target: AppController
        function onCurrentPageChanged() {
            if (AppController.currentPage.length > 0 && window.page !== AppController.currentPage)
                window.page = AppController.currentPage
        }
        function onHighlightTargetChanged() {
            const targetId = AppController.highlightTarget
            if (!targetId || targetId.length === 0)
                return
            if (targetId.indexOf("__size__:") === 0) {
                const dims = targetId.substring("__size__:".length).split("x")
                if (dims.length === 2) {
                    window.width = parseInt(dims[0], 10)
                    window.height = parseInt(dims[1], 10)
                }
                AppController.clearHighlight()
                return
            }
            if (targetId.indexOf("__grab__:") === 0) {
                const outPath = targetId.substring("__grab__:".length)
                AppController.clearHighlight()
                if (outPath.length > 0) {
                    window.grabToFile(outPath)
                }
                return
            }
            window.applyHighlightNavigation(targetId)
        }
    }
    Timer {
        id: highlightClearTimer
        interval: 2000
        onTriggered: AppController.clearHighlight()
    }
    function applyHighlightNavigation(targetId) {
        if (!targetId || targetId.length === 0)
            return
        highlightClearTimer.restart()
        if (targetId === "doctor:open") {
            AppController.runDoctor()
            return
        }
        if (targetId.indexOf("settings:") === 0) {
            const parts = targetId.split(":")
            const section = parts.length > 1 ? parts[1] : "connection"
            window.pendingSettingsSection = section
            window.page = "settings"
            if (pageLoader.item && "currentSection" in pageLoader.item) {
                pageLoader.item.currentSection = section
                window.pendingSettingsSection = ""
            }
            return
        }
        if (targetId.indexOf("messages:") === 0) {
            window.page = "messages"
            return
        }
        if (targetId.indexOf("calls:") === 0) {
            window.page = "calls"
            return
        }
        if (targetId.indexOf("photos:") === 0) {
            window.page = "photos"
            return
        }
        if (targetId.indexOf("deck:") === 0) {
            window.page = "deck"
            return
        }
        if (targetId.indexOf("apps:") === 0) {
            window.page = "apps"
            return
        }
        if (targetId.indexOf("timeline:") === 0) {
            window.page = "timeline"
            return
        }
        window.page = "home"
    }
    function pageUrl(name) {
        switch (name) {
        case "timeline": return "qrc:/qt/qml/app/g7link/qml/pages/TimelinePage.qml"
        case "messages": return "qrc:/qt/qml/app/g7link/qml/pages/MessagesPage.qml"
        case "calls": return "qrc:/qt/qml/app/g7link/qml/pages/CallsPage.qml"
        case "photos": return "qrc:/qt/qml/app/g7link/qml/pages/PhotosPage.qml"
        case "apps": return "qrc:/qt/qml/app/g7link/qml/pages/AppsPage.qml"
        case "deck": return "qrc:/qt/qml/app/g7link/qml/pages/DeckPage.qml"
        case "guide": return "qrc:/qt/qml/app/g7link/qml/pages/FeaturesGuidePage.qml"
        case "settings": return "qrc:/qt/qml/app/g7link/qml/pages/SettingsPage.qml"
        default: return "qrc:/qt/qml/app/g7link/qml/pages/HomePage.qml"
        }
    }

    // Hooks a loaded page up to the window.
    function wirePage(item) {
        if ("deviceId" in item)
            item.deviceId = Qt.binding(() => window.currentDeviceId)
        if ("deviceName" in item)
            item.deviceName = Qt.binding(() => window.currentDeviceName)
        item.active = true
        if (window.pendingSettingsSection.length > 0 && "currentSection" in item) {
            item.currentSection = window.pendingSettingsSection
            window.pendingSettingsSection = ""
        }
        if (item.pairRequested)
            item.pairRequested.connect(window.openPairing)
        if (item.showMeRequested)
            item.showMeRequested.connect(window.navigateAndHighlight)
        if (item.restartWalkthroughRequested)
            item.restartWalkthroughRequested.connect(() => Preferences.restartWalkthrough())
        if (item.textRequested) {
            item.textRequested.connect((number, name) => {
                Messages.startChat(window.currentDeviceId, number, name)
                window.page = "messages"
            })
        }
    }

    function navigateAndHighlight(targetId) {
        if (!targetId || targetId.length === 0)
            return
        AppController.triggerHighlight(targetId)
        window.applyHighlightNavigation(targetId)
    }
    readonly property int currentDeviceIndex: DeviceList.count > 0
        ? Math.min(Math.max(0, AppController.currentDevice), DeviceList.count - 1) : 0
    readonly property string currentDeviceId: DeviceList.count > 0
        ? DeviceList.deviceIdAt(currentDeviceIndex) : ""
    readonly property string currentDeviceName: DeviceList.count > 0
        ? DeviceList.deviceNameAt(currentDeviceIndex) : ""

    minimumWidth: Theme.layout.minWindow[0]
    minimumHeight: Theme.layout.minWindow[1]
    property string normalScreen: Preferences.windowScreen
    property int normalWidth: Math.max(minimumWidth, Preferences.windowWidth > 0 ? Preferences.windowWidth : 1120)
    property int normalHeight: Math.max(minimumHeight, Preferences.windowHeight > 0 ? Preferences.windowHeight : 740)
    property int normalX: Preferences.windowX
    property int normalY: Preferences.windowY
    property bool hasNormalPosition: Preferences.hasWindowPosition
    property bool restoringGeometry: true

    width: normalWidth
    height: normalHeight
    title: "G7 Link"

    function screenKey(s) {
        if (!s) return ""
        const parts = [s.manufacturer || "", s.model || "", s.serialNumber || ""].filter(function(p) { return p.length > 0 })
        if (parts.length > 0) return parts.join(":")
        return s.name || ""
    }

    function findScreen(ident) {
        const screens = Qt.application.screens
        if (!ident || ident.length === 0 || !screens || screens.length === 0) return null
        for (let i = 0; i < screens.length; ++i) {
            const s = screens[i]
            if (screenKey(s) === ident || s.name === ident) {
                return s
            }
        }
        return null
    }

    function primaryScreen() {
        const screens = Qt.application.screens
        if (screens && screens.length > 0) return screens[0]
        return window.screen ? window.screen : null
    }

    function screenOriginX(s) {
        return (s && s.virtualX !== undefined) ? s.virtualX : 0
    }

    function screenOriginY(s) {
        return (s && s.virtualY !== undefined) ? s.virtualY : 0
    }

    // A screen's own size. Its desktopAvailable* properties cover the whole
    // virtual desktop, which spans every monitor.
    function screenAvailWidth(s) {
        if (s && s.width > 0) return s.width
        return Screen.width > 0 ? Screen.width : 1920
    }

    function screenAvailHeight(s) {
        if (s && s.height > 0) return s.height
        return Screen.height > 0 ? Screen.height : 1080
    }

    function currentScreen() {
        const screens = Qt.application.screens
        if (screens && screens.length > 0) {
            const cx = window.x + Math.round(window.width / 2)
            const cy = window.y + Math.round(window.height / 2)
            for (let i = 0; i < screens.length; ++i) {
                const s = screens[i]
                const sx = screenOriginX(s)
                const sy = screenOriginY(s)
                const sw = screenAvailWidth(s)
                const sh = screenAvailHeight(s)
                if (cx >= sx && cx < sx + sw && cy >= sy && cy < sy + sh) {
                    return s
                }
            }
        }
        return window.screen ? window.screen : primaryScreen()
    }

    function updateNormalPositionFromWindow() {
        const s = currentScreen()
        const key = screenKey(s)
        if (key.length > 0) {
            normalScreen = key
        }
        normalX = window.x - screenOriginX(s)
        normalY = window.y - screenOriginY(s)
        hasNormalPosition = true
    }

    Timer {
        id: restoreCompleteTimer
        interval: 180
        onTriggered: {
            if (!Preferences.windowMaximized && window.visibility === Window.Windowed) {
                const s = findScreen(window.normalScreen) || primaryScreen()
                const targetGlobalX = screenOriginX(s) + window.normalX
                const targetGlobalY = screenOriginY(s) + window.normalY
                // In case showing changed it (see restoreAndBringToFront).
                if (window.width !== window.normalWidth) window.width = window.normalWidth
                if (window.height !== window.normalHeight) window.height = window.normalHeight
                if (window.hasNormalPosition) {
                    if (window.x !== targetGlobalX) window.x = targetGlobalX
                    if (window.y !== targetGlobalY) window.y = targetGlobalY
                } else if (window.visible) {
                    window.updateNormalPositionFromWindow()
                }
            }
            window.restoringGeometry = false
        }
    }

    Timer {
        id: geometrySaveTimer
        interval: 250
        onTriggered: window.saveWindowState()
    }

    // Note: On Wayland compositors, client-side global window coordinates (x, y) read as 0
    // and setting them is a no-op; only window dimensions, target screen, and maximized state take effect.
    function restoreAndBringToFront() {
        restoringGeometry = true
        const savedScreen = findScreen(Preferences.windowScreen)
        const targetScreen = savedScreen ? savedScreen : primaryScreen()
        const sx = screenOriginX(targetScreen)
        const sy = screenOriginY(targetScreen)
        const availW = screenAvailWidth(targetScreen)
        const availH = screenAvailHeight(targetScreen)

        const desiredW = Math.max(minimumWidth, Preferences.windowWidth > 0 ? Preferences.windowWidth : 1120)
        const desiredH = Math.max(minimumHeight, Preferences.windowHeight > 0 ? Preferences.windowHeight : 740)
        const minW = Math.min(minimumWidth, availW)
        const minH = Math.min(minimumHeight, availH)
        const fittedW = Math.max(minW, Math.min(desiredW, availW))
        const fittedH = Math.max(minH, Math.min(desiredH, availH))

        normalWidth = fittedW
        normalHeight = fittedH
        width = fittedW
        height = fittedH

        const targetKey = screenKey(targetScreen)
        if (targetKey.length > 0) {
            normalScreen = targetKey
        }

        if (savedScreen && Preferences.hasWindowPosition) {
            const maxRelX = Math.max(0, availW - fittedW)
            const maxRelY = Math.max(0, availH - fittedH)
            normalX = Math.max(0, Math.min(Preferences.windowX, maxRelX))
            normalY = Math.max(0, Math.min(Preferences.windowY, maxRelY))
            hasNormalPosition = true
        } else {
            normalX = Math.max(0, Math.round((availW - fittedW) / 2))
            normalY = Math.max(0, Math.round((availH - fittedH) / 2))
            hasNormalPosition = true
        }

        // NativeWindow's QML type doesn't expose `screen`; a position in
        // desktop coordinates puts the window on the right monitor anyway.
        x = sx + normalX
        y = sy + normalY

        bringToFront()
        // Again once shown: a size set before the first show can come out
        // taller by the title bar. This is also the size
        // un-maximizing returns to.
        width = fittedW
        height = fittedH
        x = sx + normalX
        y = sy + normalY
        if (Preferences.windowMaximized)
            showMaximized()
        restoreCompleteTimer.restart()
    }

    function toggleFullScreen() {
        if (window.visibility !== Window.FullScreen)
            showFullScreen()
        else if (Preferences.windowMaximized)
            showMaximized()
        else
            showNormal()
    }

    function saveWindowState() {
        geometrySaveTimer.stop()
        if (window.visible && !restoringGeometry) {
            const s = currentScreen()
            const key = screenKey(s)
            if (key.length > 0) {
                normalScreen = key
            }
            if (window.visibility === Window.Windowed
                && window.width >= window.minimumWidth && window.height >= window.minimumHeight) {
                normalX = window.x - screenOriginX(s)
                normalY = window.y - screenOriginY(s)
                hasNormalPosition = true
                normalWidth = Math.max(window.minimumWidth, window.width)
                normalHeight = Math.max(window.minimumHeight, window.height)
            }
        }
        Preferences.saveWindowGeometry(
            normalScreen,
            hasNormalPosition,
            normalX,
            normalY,
            normalWidth,
            normalHeight,
            window.visibility === Window.Maximized || window.visibility === Window.FullScreen
        )
    }

    property bool resizing: resizeSettleTimer.running
    Timer {
        id: resizeSettleTimer
        interval: 140
    }
    onXChanged: if (visible && !restoringGeometry) geometrySaveTimer.restart()
    onYChanged: if (visible && !restoringGeometry) geometrySaveTimer.restart()
    onWidthChanged: {
        if (visible) {
            resizeSettleTimer.restart()
            if (!restoringGeometry)
                geometrySaveTimer.restart()
        }
    }
    onHeightChanged: {
        if (visible) {
            resizeSettleTimer.restart()
            if (!restoringGeometry)
                geometrySaveTimer.restart()
        }
    }
    // QWindow::visibilityChanged is a revisioned signal that NativeWindow's
    // QML type doesn't expose as a handler, so connect to it instead.
    Connections {
        target: window
        function onVisibilityChanged() {
            if (window.visible && !window.restoringGeometry
                && (window.visibility === Window.Windowed || window.visibility === Window.Maximized))
                geometrySaveTimer.restart()
            window.updateInteractive()
        }
    }
    onVisibleChanged: updateInteractive()

    // Links to devices are checked every 5 s instead of 15 s while this
    // window is showing, so one that dropped shows sooner.
    function updateInteractive() {
        AppController.setInteractive(window.visible && window.visibility !== Window.Minimized
                                     && window.visibility !== Window.Hidden)
    }

    Connections {
        target: window
        function onActiveChanged() {
            if (window.active) {
                AppController.refreshToastsEnabled()
                if (!window.activeFocusItem && window.contentItem)
                    window.contentItem.forceActiveFocus()
            }
        }
        function onActiveFocusItemChanged() {
            if (!window.activeFocusItem && window.contentItem)
                Qt.callLater(() => {
                    if (!window.activeFocusItem && window.contentItem)
                        window.contentItem.forceActiveFocus()
                })
        }
    }
    Component.onCompleted: {
        updateInteractive()
        if (!window.activeFocusItem && window.contentItem)
            window.contentItem.forceActiveFocus()
        if (currentDeviceId.length > 0)
            AppController.watchHomeSummary(currentDeviceId)
    }
    darkFrame: Theme.dark
    captionHeight: Theme.topBarHeight

    function openPairing() { pairingSheet.open() }

    // Window background: the theme's surface, tinted with its seed color.
    readonly property color windowSurface: Theme.graphite || Theme.amoled || Theme.hive || Theme.quiet
        ? Theme.surface
        : Qt.tint(
            Theme.surface,
            Qt.rgba(
                Theme.primaryContainer.r,
                Theme.primaryContainer.g,
                Theme.primaryContainer.b,
                Theme.isPhoneSeed ? (Theme.dark ? 0.22 : 0.30) : (Theme.dark ? 0.12 : 0.18)
            )
        )
    Rectangle {
        anchors.fill: parent
        color: window.windowSurface
        Behavior on color { ColorAnimation { duration: Theme.fadeNormal } }
    }

    // Hive: the living comb behind everything, growing from the logo. It
    // holds its frame while the window isn't in front or is minimized.
    HiveBackdrop {
        id: hiveComb
        anchors.fill: parent
        visible: opacity > 0
        opacity: Theme.hive ? 1 : 0
        Behavior on opacity {
            enabled: !Theme.reduceMotion
            NumberAnimation { duration: 420; easing.type: Easing.OutCubic }
        }
        animate: Theme.combMotion
        ownClock: false
        time: Theme.combTime
        cell: Theme.comb.cell
        frameMs: Theme.comb.frameMs
        breathSeconds: Theme.comb.breathSeconds
        pulseSeconds: Theme.comb.pulseSeconds
        lineColor: Theme.combLine
        glowColor: Theme.combGlow
        // The G7 mark in the footer; the comb and its pulses carry across
        // the whole window.
        origin: Qt.point(28, window.height - Theme.footerHeight / 2)
        reach: 2.4
        pulseReach: 1.05
    }

    // The comb's clock runs while this window is in front and showing Hive.
    Binding {
        target: Theme
        property: "combClockRunning"
        value: window.active && window.visible
               && window.visibility !== Window.Minimized && window.visibility !== Window.Hidden
    }

    // Dynamic native GPU-blurred ambient layer when "Glass" theme mode is active.
    Item {
        id: artworkAmbient
        anchors.fill: parent
        clip: true
        opacity: Theme.ambientBlurActive ? 1.0 : 0.0
        visible: opacity > 0
        Behavior on opacity {
            enabled: !Theme.reduceMotion
            NumberAnimation { duration: 420; easing.type: Easing.OutCubic }
        }

        // Fixed-resolution 256x256 ambient mesh + blur so window resizing never
        // reallocates full-window GPU blur FBOs on every pixel.
        Item {
            id: glassColorMesh
            width: 256
            height: 256
            visible: false
            layer.enabled: glassMeshBlur.visible && window.visible && window.visibility !== Window.Minimized
            layer.textureSize: Qt.size(256, 256)
            layer.smooth: true

            Rectangle {
                width: 174; height: 174; radius: 87
                x: -31; y: -42
                color: Theme.primary
                opacity: Theme.dark ? 0.56 : 0.44
                Behavior on color { ColorAnimation { duration: 480 } }
            }
            Rectangle {
                width: 154; height: 154; radius: 77
                x: 123; y: 56
                color: Theme.primaryContainer
                opacity: Theme.dark ? 0.62 : 0.52
                Behavior on color { ColorAnimation { duration: 480 } }
            }
            Rectangle {
                width: 138; height: 138; radius: 69
                x: 31; y: 128
                color: Theme.secondaryContainer
                opacity: Theme.dark ? 0.54 : 0.46
                Behavior on color { ColorAnimation { duration: 480 } }
            }
            Rectangle {
                width: 108; height: 108; radius: 54
                x: 159; y: -15
                color: Qt.lighter(Theme.primary, 1.35)
                opacity: Theme.dark ? 0.34 : 0.28
                Behavior on color { ColorAnimation { duration: 480 } }
            }
        }

        MultiEffect {
            id: glassMeshBlur
            width: 256
            height: 256
            source: glassColorMesh
            visible: Theme.glass
            autoPaddingEnabled: false
            blurEnabled: true
            blurMax: 48
            blur: 1.0
            saturation: Theme.dark ? 0.42 : 0.30
            transform: Scale {
                xScale: artworkAmbient.width / 256
                yScale: artworkAmbient.height / 256
            }
        }

        // Translucent surface veil so UI cards and typography stay crisp over the blurred backdrop.
        Rectangle {
            anchors.fill: parent
            color: Qt.rgba(
                Theme.surface.r,
                Theme.surface.g,
                Theme.surface.b,
                Theme.glass ? (Theme.amoled ? 0.62 : (Theme.dark ? 0.22 : 0.28)) : (Theme.dark ? 0.60 : 0.66)
            )
            Behavior on color {
                enabled: !Theme.reduceMotion
                ColorAnimation { duration: 360; easing.type: Easing.OutCubic }
            }
        }
    }

    // ---- The window: a header, a sidebar and a footer around the page ----
    // Every page can describe its footer and its Actions list:
    //   footerLabel: "Photos · 1,284" (else the page's name)
    //   primaryAction / secondaryAction: { label, keys, run }
    //   screenActions: [{ label, detail, icon, tint, keys, checked, enabled, run }]
    // The page itself handles the keys it advertises.
    readonly property var pageItem: pageLoader.status === Loader.Ready ? pageLoader.item : null
    readonly property var primaryAction: pageItem && pageItem.primaryAction ? pageItem.primaryAction : null
    readonly property var secondaryAction: pageItem && pageItem.secondaryAction ? pageItem.secondaryAction : null
    readonly property var screenActions: pageItem && pageItem.screenActions ? pageItem.screenActions : []
    readonly property var navItems: [
        { page: "home", icon: Icons.home, tint: Theme.tintLavender, name: qsTr("Home") },
        { page: "messages", icon: Icons.messages, tint: Theme.tintBlue, name: qsTr("Messages") },
        { page: "calls", icon: Icons.call, tint: Theme.tintTeal, name: qsTr("Calls") },
        { page: "photos", icon: Icons.photo, tint: Theme.tintCoral, name: qsTr("Photos") },
        { page: "apps", icon: Icons.apps, tint: Theme.tintPink, name: qsTr("Apps") },
        { page: "deck", icon: Icons.deck, tint: Theme.tintGreen, name: qsTr("Deck") },
        { page: "timeline", icon: Icons.history, tint: Theme.tintLavender, name: qsTr("Timeline") },
        { page: "settings", icon: Icons.settings, tint: Theme.surfaceContentVariant, name: qsTr("Settings") }
    ]
    readonly property string pageName: {
        if (window.page === "guide")
            return qsTr("Guide")
        const item = window.navItems.find((n) => n.page === window.page)
        return item ? item.name : ""
    }
    // Unread messages and missed calls on the phone on screen, for the sidebar.
    readonly property var homeSummary: {
        AppController.homeSummaryRevision
        if (window.currentDeviceId.length === 0)
            return ({})
        try {
            return JSON.parse(AppController.homeSummary(window.currentDeviceId) || "{}")
        } catch (e) {
            return ({})
        }
    }
    onCurrentDeviceIdChanged: if (currentDeviceId.length > 0) AppController.watchHomeSummary(currentDeviceId)
    function navCount(page) {
        const n = page === "messages" ? window.homeSummary.unreadMessages
            : (page === "calls" ? window.homeSummary.missedCalls : 0)
        return n > 0 ? String(n) : ""
    }

    // The phone on screen: its link and battery, for the footer.
    Instantiator {
        id: deviceRows
        model: DeviceList
        delegate: QtObject {
            required property int index
            required property string path
            required property int rttMs
            required property int battery
            required property bool charging
            required property bool online
        }
    }
    readonly property var currentRow: deviceRows.count > 0 ? deviceRows.objectAt(window.currentDeviceIndex) : null
    readonly property string linkText: {
        const d = window.currentRow
        if (!d)
            return ""
        const name = d.path === "usb" ? "USB" : (d.path === "lan" ? qsTr("Wi-Fi") : (d.path === "relay" ? qsTr("Internet") : ""))
        if (!d.online)
            return d.path === "connecting" ? qsTr("Connecting") : qsTr("Offline")
        return d.rttMs >= 0 ? qsTr("%1 · %2 ms").arg(name).arg(d.rttMs) : name
    }

    function deviceMenuActions() {
        const list = []
        for (let i = 0; i < DeviceList.count; ++i) {
            const index = i
            list.push({
                label: DeviceList.deviceNameAt(i),
                icon: Icons.phone,
                tint: index === window.currentDeviceIndex ? Theme.accent : Theme.surfaceContentVariant,
                checked: index === window.currentDeviceIndex,
                run: () => { AppController.currentDevice = index }
            })
        }
        list.push({ label: qsTr("Pair a phone"), icon: Icons.plus, tint: Theme.accent, run: () => window.openPairing() })
        return list
    }
    readonly property var g7MenuActions: [
        { label: qsTr("Guide"), icon: Icons.book, tint: Theme.tintBlue, run: () => { window.page = "guide" } },
        { label: qsTr("Pair a phone"), icon: Icons.plus, tint: Theme.accent, run: () => window.openPairing() },
        { label: qsTr("Check this PC"), icon: Icons.doctor, tint: Theme.tintTeal, run: () => AppController.runDoctor() },
        { label: qsTr("Command palette"), icon: Icons.search, tint: Theme.surfaceContentVariant, keys: "Ctrl K",
          run: () => AppController.openCommandPalette() },
        { label: qsTr("Quit G7 Link"), icon: Icons.power, tint: Theme.tintRed, run: () => {
            window.saveWindowState()
            Qt.quit()
        } }
    ]
    function openActions() {
        const list = window.screenActions.slice()
        if (list.length === 0)
            return
        actionsMenu.actions = list
        actionsMenu.openAt(footer.width - 14, footer.y, true, true)
    }

    // ---- Header: the phone picker, then search ----
    Item {
        id: header
        visible: !window.welcome
        anchors.left: parent.left
        anchors.right: captionButtons.left
        height: Theme.headerHeight

        Rectangle {
            id: phonePicker
            x: 12
            anchors.verticalCenter: parent.verticalCenter
            width: pickerRow.implicitWidth + 16
            height: 32
            radius: Theme.rowRadius
            color: pickerMouse.containsMouse || phoneMenu.opened ? Theme.raised : "transparent"
            Accessible.role: Accessible.ButtonMenu
            Accessible.name: qsTr("Phone: %1").arg(window.currentDeviceName)
            Row {
                id: pickerRow
                anchors.centerIn: parent
                spacing: 8
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 18; height: 18
                    path: Icons.phone
                    color: Theme.accent
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.min(implicitWidth, 220)
                    text: window.currentDeviceName.length > 0 ? window.currentDeviceName : qsTr("Pair a phone")
                    weight: 500
                }
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14; height: 14
                    path: Icons.chevronDown
                    color: Theme.surfaceContentVariant
                }
            }
            MouseArea {
                id: pickerMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    phoneMenu.actions = window.deviceMenuActions()
                    phoneMenu.openAt(phonePicker.x, header.height - 6, false, false)
                }
            }
            Component.onCompleted: window.addCaptionHole(phonePicker)
        }
        Field {
            id: headerSearch
            anchors.left: phonePicker.right
            anchors.leftMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(parent.width - x - 16, 720)
            readOnly: true
            iconPath: Icons.search
            placeholder: qsTr("Search messages, contacts, apps and actions")
            keys: Preferences.commandPaletteHotkey.length > 0 ? Preferences.commandPaletteHotkey : "Ctrl K"
            onPressed: AppController.openCommandPalette()
            Component.onCompleted: window.addCaptionHole(headerSearch)
        }
    }
    Rectangle {
        visible: header.visible
        y: Theme.headerHeight - 1
        width: parent.width
        height: 1
        color: Theme.line
    }

    // ---- Sidebar ----
    Item {
        id: sidebar
        visible: !window.welcome
        y: Theme.headerHeight
        width: window.width < 1000 ? 52 : Theme.sidebarWidth
        height: parent.height - Theme.headerHeight - Theme.footerHeight
        readonly property bool iconsOnly: width < Theme.sidebarWidth

        Column {
            anchors.fill: parent
            anchors.margins: sidebar.iconsOnly ? 4 : 8
            spacing: 2
            Repeater {
                model: window.navItems
                ActionRow {
                    required property var modelData
                    width: parent.width
                    iconPath: modelData.icon
                    tint: modelData.tint
                    title: sidebar.iconsOnly ? "" : modelData.name
                    accessibleName: modelData.name
                    meta: sidebar.iconsOnly ? "" : window.navCount(modelData.page)
                    selected: window.page === modelData.page
                    onClicked: window.page = modelData.page
                }
            }
        }
        Rectangle {
            anchors.right: parent.right
            width: 1
            height: parent.height
            color: Theme.line
        }
    }

    // ---- Pages ----
    Item {
        id: main
        visible: !window.welcome
        anchors.left: sidebar.right
        anchors.right: parent.right
        y: Theme.headerHeight
        height: parent.height - Theme.headerHeight - Theme.footerHeight

        Rectangle {
            id: pages
            anchors.fill: parent
            clip: true
            // Hive and Glass let their backdrop show faintly through.
            color: Theme.hive || Theme.glass
                ? Qt.rgba(Theme.surface.r, Theme.surface.g, Theme.surface.b, Theme.dark ? 0.50 : 0.58)
                : (Theme.amoled ? "#000000" : Theme.surface)

            Loader {
                id: pageLoader
                anchors.fill: parent
                active: !window.welcome && AppController.status === "ready"
                source: active ? window.pageUrl(window.page) : ""
                onLoaded: {
                    window.wirePage(item)
                    if (!Theme.reduceMotion) {
                        pageLoader.opacity = 0.4
                        pageFade.restart()
                    }
                }
                NumberAnimation {
                    id: pageFade
                    target: pageLoader
                    property: "opacity"
                    to: 1.0
                    duration: Theme.fadeFast
                    easing.type: Easing.OutCubic
                }
            }
        }
    }

    // ---- Footer ----
    Item {
        id: footer
        visible: !window.welcome
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: Theme.footerHeight

        Rectangle {
            width: parent.width
            height: 1
            color: Theme.line
        }
        Row {
            id: footerLeft
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 14

            Rectangle {
                id: mark
                anchors.verticalCenter: parent.verticalCenter
                width: 20
                height: 20
                color: "transparent"
                Accessible.role: Accessible.ButtonMenu
                Accessible.name: qsTr("G7 Link menu")
                Icon {
                    anchors.fill: parent
                    path: Icons.mark
                    color: Theme.accent
                    stroke: 2
                }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: g7Menu.openAt(footerLeft.x, footer.y, true, false)
                }
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                visible: window.page !== "home" && text.length > 0
                text: window.pageItem && window.pageItem.footerLabel ? window.pageItem.footerLabel : window.pageName
                role: "bodySmall"
                muted: true
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                visible: text.length > 0 && footer.width > 760
                text: window.linkText
                role: "bodySmall"
                muted: true
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                visible: window.currentRow !== null && window.currentRow.battery >= 0
                text: window.currentRow ? (window.currentRow.charging ? qsTr("%1% · charging") : "%1%").arg(window.currentRow.battery) : ""
                role: "bodySmall"
                color: Theme.batteryColor(window.currentRow ? window.currentRow.battery : -1)
            }
            // This PC's screen is being shared: say so, with a way to stop.
            Row {
                anchors.verticalCenter: parent.verticalCenter
                visible: AppController.pcMirrorActive
                spacing: 8
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 6; height: 6; radius: 3
                    color: Theme.tintRed
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: AppController.pcMirrorDeviceName.length > 0
                        ? qsTr("Sharing screen with %1").arg(AppController.pcMirrorDeviceName)
                        : qsTr("Sharing screen")
                    role: "bodySmall"
                    color: Theme.tintRed
                }
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Stop")
                    Accessible.name: qsTr("Stop sharing this PC's screen")
                    onClicked: AppController.stopPcMirror()
                }
            }
        }
        Row {
            anchors.right: parent.right
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10

            FooterAction {
                action: window.primaryAction
                strong: true
            }
            FooterAction {
                action: window.secondaryAction
            }
            FooterAction {
                visible: window.screenActions.length > 0
                action: ({ label: qsTr("Actions"), keys: "Ctrl B", run: () => window.openActions() })
            }
        }
    }
    component FooterAction: Row {
        id: fa
        property var action: null
        property bool strong: false
        visible: action !== null
        spacing: 6
        height: 28
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: faText.implicitWidth + 12
            height: 24
            radius: Theme.radiusSm
            color: faMouse.containsMouse ? Theme.raised : "transparent"
            Txt {
                id: faText
                anchors.centerIn: parent
                text: fa.action ? fa.action.label : ""
                role: "bodySmall"
                muted: !fa.strong
            }
            MouseArea {
                id: faMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: if (fa.action && typeof fa.action.run === "function") fa.action.run()
            }
            Accessible.role: Accessible.Button
            Accessible.name: faText.text
        }
        Keycap {
            anchors.verticalCenter: parent.verticalCenter
            keys: fa.action && fa.action.keys ? fa.action.keys : ""
        }
    }

    PopupMenu {
        id: phoneMenu
        panelWidth: 280
    }
    PopupMenu {
        id: g7Menu
        panelWidth: 260
        actions: window.g7MenuActions
    }
    PopupMenu {
        id: actionsMenu
        filterable: true
        panelWidth: 340
    }
    Shortcut {
        sequence: "Ctrl+B"
        enabled: !window.welcome
        onActivated: {
            if (actionsMenu.opened)
                actionsMenu.close()
            else
                window.openActions()
        }
    }

    StarPrompt {
        eligible: AppController.status === "ready" && !window.showWalkthrough && window.active
        onStarred: Qt.openUrlExternally("https://github.com/Astraque-Softwares-Ltd/g7link")
    }

    // ---- First run ----
    Loader {
        anchors.fill: parent
        active: AppController.status !== "ready"
        source: active ? "qrc:/qt/qml/app/g7link/qml/pages/WelcomePage.qml" : ""
    }

    Loader {
        id: walkthroughLoader
        anchors.fill: parent
        active: window.showWalkthrough
        source: active ? "qrc:/qt/qml/app/g7link/qml/pages/WalkthroughPage.qml" : ""
        onLoaded: {
            if ("deviceId" in item)
                item.deviceId = Qt.binding(() => window.currentDeviceId)
            if ("deviceName" in item)
                item.deviceName = Qt.binding(() => window.currentDeviceName)
            item.completed.connect(() => { window.page = "home" })
            item.openGuideRequested.connect(() => { window.page = "guide" })
        }
    }

    // ---- Overlays ----
    Connections {
        target: Pairing
        function onStateChanged() {
            if (Pairing.state === "comparing" && !window.welcome && !pairingSheet.opened) {
                pairingSheet.open()
            }
        }
    }
    Sheet {
        id: pairingSheet
        cardWidth: 760
        onOpenedChanged: if (!opened) Pairing.cancel()
        Loader {
            id: pairingLoader
            width: parent.width
            active: pairingSheet.opened
            source: active ? "qrc:/qt/qml/app/g7link/qml/pages/PairingPanel.qml" : ""
            onLoaded: {
                item.width = Qt.binding(() => pairingLoader.width)
                item.running = Qt.binding(() => pairingSheet.opened)
                item.finished.connect(() => pairingSheet.close())
                item.cancelled.connect(() => pairingSheet.close())
            }
        }
    }

    Sheet {
        id: ringingSheet
        opened: AppController.ringingFrom.length > 0
        dismissable: false
        cardWidth: 380
        Column {
            width: parent.width
            spacing: 16
            Avatar { anchors.horizontalCenter: parent.horizontalCenter; width: 56; height: 56; iconPath: Icons.ring; emphasized: true }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: qsTr("%1 is ringing this PC").arg(AppController.ringingFrom)
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Button {
                anchors.horizontalCenter: parent.horizontalCenter
                text: qsTr("Stop ringing")
                onClicked: AppController.stopRinging()
            }
        }
    }

    Sheet {
        id: remotePromptSheet
        opened: AppController.remotePromptDeviceId.length > 0
        cardWidth: 420
        onOpenedChanged: {
            if (!opened && AppController.remotePromptDeviceId.length > 0)
                AppController.dismissRemotePrompt()
        }
        Column {
            width: parent.width
            spacing: 16
            Avatar {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 56; height: 56
                iconPath: Icons.mouse
                emphasized: true
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: qsTr("Allow %1 to control this PC?").arg(AppController.remotePromptDeviceName)
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: qsTr("%1 wants to use this PC's mouse and keyboard. You can change this in Settings.").arg(AppController.remotePromptDeviceName)
                role: "body"
                muted: true
                wrapMode: Text.WordWrap
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Not now")
                    onClicked: AppController.dismissRemotePrompt()
                }
                Button {
                    text: qsTr("Allow")
                    onClicked: AppController.allowRemoteInput(AppController.remotePromptDeviceId)
                }
            }
        }
    }

    Sheet {
        id: pcMirrorPromptSheet
        opened: AppController.pcMirrorPromptDeviceId.length > 0
        cardWidth: 460
        onOpenedChanged: {
            if (!opened && AppController.pcMirrorPromptDeviceId.length > 0)
                AppController.dismissPcMirrorPrompt()
        }
        Column {
            width: parent.width
            spacing: 16
            Avatar {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 56; height: 56
                iconPath: Icons.mirror
                emphasized: true
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: qsTr("Allow %1 to view this PC's screen?").arg(AppController.pcMirrorPromptDeviceName)
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: qsTr("%1 wants to view this PC's screen. Control needs Remote mouse, keyboard & Deck in Settings.").arg(AppController.pcMirrorPromptDeviceName)
                role: "body"
                muted: true
                wrapMode: Text.WordWrap
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Not now")
                    onClicked: AppController.dismissPcMirrorPrompt()
                }
                Button {
                    variant: "tonal"
                    text: qsTr("Allow")
                    onClicked: AppController.allowPcMirrorOnce(AppController.pcMirrorPromptDeviceId)
                }
                Button {
                    text: qsTr("Allow for this device")
                    onClicked: AppController.allowPcMirrorDevice(AppController.pcMirrorPromptDeviceId)
                }
            }
        }
    }

    Sheet {
        id: continuityPasteSheet
        opened: AppController.continuityPastePromptVisible
        cardWidth: 440
        onOpenedChanged: {
            if (!opened && AppController.continuityPastePromptVisible)
                AppController.dismissContinuityPaste()
        }
        Column {
            width: parent.width
            spacing: 16
            Avatar {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 56; height: 56
                iconPath: Icons.camera
                emphasized: true
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: AppController.continuityPastePromptTitle.length > 0
                    ? AppController.continuityPastePromptTitle
                    : qsTr("Photo ready on clipboard")
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: AppController.continuityPastePromptBody
                role: "body"
                muted: true
                wrapMode: Text.WordWrap
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Keep on clipboard")
                    onClicked: AppController.dismissContinuityPaste()
                }
                Button {
                    text: qsTr("Paste into active window")
                    onClicked: AppController.confirmContinuityPaste()
                }
            }
        }
    }

    CaptionButtons {
        id: captionButtons
        window: window
        anchors.top: parent.top
        anchors.right: parent.right
    }

    Loader {
        id: doctorLoader
        anchors.fill: parent
        active: false
        source: active ? "qrc:/qt/qml/app/g7link/qml/components/DoctorSheet.qml" : ""
        onLoaded: item.open()
    }
    Connections {
        target: AppController
        function onDoctorRequested() {
            if (doctorLoader.active && doctorLoader.item)
                doctorLoader.item.open()
            else
                doctorLoader.active = true
        }
    }

    // `--ui-check`: visit every page, the menus, the palette, the Shelf,
    // the Island and the welcome screen once, so a QML error on any of them
    // fails the check.
    readonly property bool uiCheck: AppController.isUiCheck()
    property bool uiCheckWelcome: false
    Timer {
        id: uiCheckTour
        property int step: 0
        readonly property var stops: ["messages", "calls", "photos", "apps", "deck", "timeline", "settings", "guide", "home"]
        interval: 450
        repeat: true
        running: window.uiCheck && AppController.status === "ready"
        onTriggered: {
            const n = stops.length
            // Info, not a warning: it shows in CI's log which stops were reached.
            console.info("ui-check: tour step", step, step < n ? stops[step] : "")
            if (step < n) {
                window.page = stops[step]
            } else if (step === n) {
                window.openActions()
            } else if (step === n + 1) {
                actionsMenu.close()
                phoneMenu.actions = window.deviceMenuActions()
                phoneMenu.openAt(12, Theme.headerHeight, false, false)
            } else if (step === n + 2) {
                phoneMenu.close()
                g7Menu.openAt(14, footer.y, true, false)
            } else if (step === n + 3) {
                g7Menu.close()
                window.openPairing()
            } else if (step === n + 4) {
                pairingSheet.close()
                AppController.openCommandPalette()
            } else if (step === n + 5) {
                AppController.closeCommandPalette()
                AppController.toggleShelf()
                Preferences.g7Island = true
            } else if (step === n + 6) {
                AppController.closeShelf()
                window.uiCheckWelcome = true
            } else {
                stop()
            }
            step++
        }
    }

    Shortcut {
        sequence: "F11"
        onActivated: window.toggleFullScreen()
    }

    Shortcut {
        sequences: ["Ctrl+K", "Ctrl+P"]
        onActivated: {
            if (AppController.commandPaletteOpen)
                AppController.closeCommandPalette()
            else
                AppController.openCommandPalette()
        }
    }

    Loader {
        id: commandPaletteLoader
        anchors.fill: parent
        active: AppController.commandPaletteOpen
        source: active ? "qrc:/qt/qml/app/g7link/qml/components/CommandPalette.qml" : ""
    }

    Toast {
        id: toast
        onActionTriggered: AppController.runClipSuggestion()
    }
    Connections {
        target: AppController
        function onToast(message) { toast.show(message) }
        function onToastWithAction(message, actionLabel) { toast.showWithAction(message, actionLabel) }
        function onPendingDialChanged() {
            if (AppController.pendingDial.length > 0 && window.page !== "calls")
                window.page = "calls"
        }
    }
}
