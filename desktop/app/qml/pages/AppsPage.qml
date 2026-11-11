// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Apps: the phone's whole screen in a window, and every app on the phone,
// to open one in a window of its own on this PC (Elevated: the phone runs
// it on a display of its own). Searchable; Enter opens the first match.
// Apps already open here are marked.
Item {
    id: page
    property bool active: false
    property string deviceId
    property string deviceName

    readonly property var feature: AppController.capsRevision >= 0 && deviceId.length > 0
        ? AppController.featureState(deviceId, "mirroring.app_windows") : ({})
    readonly property var mirrorFeature: AppController.capsRevision >= 0 && deviceId.length > 0
        ? AppController.featureState(deviceId, "mirroring.view") : ({})
    readonly property bool available: feature.state === "available" || feature.state === "partial"
    readonly property bool mirrorLocked: mirrorFeature.state !== undefined
        && mirrorFeature.state !== "available" && mirrorFeature.state !== "partial"

    readonly property var apps: {
        if (Mirror.appsDevice !== deviceId)
            return []
        try { return JSON.parse(Mirror.apps) } catch (e) { return [] }
    }
    readonly property var recent: {
        if (Mirror.appsDevice !== deviceId)
            return []
        try { return JSON.parse(Mirror.recentApps) } catch (e) { return [] }
    }
    // Packages with a window open on this PC.
    readonly property var openPkgs: {
        try {
            const open = {}
            JSON.parse(Mirror.windows).forEach(w => {
                if (w.device === page.deviceId && w.app && w.pkg && w.phase !== "ended")
                    open[w.pkg] = true
            })
            return open
        } catch (e) {
            return ({})
        }
    }
    readonly property string query: search.text.trim().toLocaleLowerCase()
    readonly property var shown: query.length === 0 ? apps
        : apps.filter(a => a.label.toLocaleLowerCase().indexOf(query) >= 0 || a.pkg.indexOf(query) >= 0)
    readonly property string loadState: Mirror.appsDevice === deviceId ? Mirror.appsState : ""
    readonly property string qualityName: Preferences.mirrorQuality === "sharper" ? qsTr("Sharper")
        : Preferences.mirrorQuality === "smoother" ? qsTr("Smoother") : qsTr("Balanced")
    // The app the keyboard is on (the grid's current cell), else the first match.
    readonly property var currentApp: shown.length === 0 ? null
        : shown[Math.max(0, Math.min(grid.currentIndex, shown.length - 1))]

    function launch(app) {
        if (app)
            Mirror.startApp(deviceId, app.pkg, app.label)
    }
    function mirror() {
        if (deviceId.length > 0)
            Mirror.start(deviceId)
    }

    // ---- Footer and Actions (Ctrl B) ----
    readonly property string footerLabel: apps.length > 0 ? qsTr("Apps · %1").arg(apps.length) : qsTr("Apps")
    readonly property var primaryAction: available && shown.length > 0
        ? { label: qsTr("Open in window"), keys: "Enter", run: () => page.launch(page.currentApp) }
        : (deviceId.length > 0 ? { label: qsTr("Mirror screen"), keys: "Ctrl M", run: () => page.mirror() } : null)
    readonly property var screenActions: {
        const list = []
        if (deviceId.length === 0)
            return list
        list.push({ label: qsTr("Mirror whole screen"), icon: Icons.phone, tint: Theme.accent, keys: "Ctrl M",
                    enabled: !page.mirrorLocked, run: () => page.mirror() })
        if (available) {
            if (currentApp)
                list.push({ label: qsTr("Open %1 in a window").arg(currentApp.label), icon: Icons.window,
                            tint: Theme.tintPink, keys: "Enter", run: () => page.launch(page.currentApp) })
            list.push({ label: qsTr("Search apps"), icon: Icons.search, tint: Theme.surfaceContentVariant,
                        keys: "Ctrl F", run: () => search.forceActiveFocus() })
            list.push({ label: qsTr("Refresh the app list"), icon: Icons.refresh, tint: Theme.tintBlue,
                        enabled: page.loadState !== "loading", run: () => Mirror.loadApps(page.deviceId) })
            for (const app of recent) {
                const a = app
                list.push({ label: qsTr("Open %1").arg(a.label), detail: qsTr("Recent"), icon: Icons.apps,
                            tint: Theme.tintPink, run: () => page.launch(a) })
            }
        }
        return list
    }

    // Loads the list the first time the page shows for a phone; Refresh reloads it.
    function loadIfNeeded() {
        if (!active || !available || deviceId.length === 0)
            return
        if (Mirror.appsDevice !== deviceId || Mirror.appsState === "" || Mirror.appsState === "failed")
            Mirror.loadApps(deviceId)
    }
    onActiveChanged: if (active) { loadIfNeeded(); search.forceActiveFocus() }
    onDeviceIdChanged: { search.text = ""; loadIfNeeded() }
    onAvailableChanged: loadIfNeeded()
    Component.onCompleted: loadIfNeeded()

    Shortcut {
        sequence: "Ctrl+M"
        enabled: page.active && page.visible && page.deviceId.length > 0
        onActivated: page.mirror()
    }
    Shortcut {
        sequences: [StandardKey.Find]
        enabled: page.active && page.visible && page.available
        onActivated: search.forceActiveFocus()
    }

    // ---- No phone ----
    Column {
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 360)
        spacing: 8
        visible: page.deviceId.length === 0
        Icon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 24; height: 24
            path: Icons.apps
            color: Theme.tintPink
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            muted: true
            text: qsTr("Pair a phone to open its apps here")
        }
    }

    Item {
        id: content
        visible: page.deviceId.length > 0
        anchors.fill: parent
        anchors.margins: 8

        // ---- The whole screen ----
        Rectangle {
            id: mirrorRow
            y: 6
            width: parent.width
            height: 44
            radius: Theme.rowRadius
            color: mirrorMouse.containsMouse && !page.mirrorLocked ? Theme.selection : Theme.raised
            border.width: Theme.focusVisible(mirrorRow) ? 2 : 0
            border.color: Theme.focusRing
            activeFocusOnTab: true
            Accessible.role: Accessible.Button
            Accessible.name: qsTr("Mirror whole screen")
            Accessible.onPressAction: page.mirror()
            Keys.onReturnPressed: page.mirror()
            Keys.onEnterPressed: page.mirror()
            Keys.onSpacePressed: page.mirror()

            MouseArea {
                id: mirrorMouse
                anchors.fill: parent
                hoverEnabled: true
                enabled: !page.mirrorLocked
                cursorShape: Qt.PointingHandCursor
                onClicked: page.mirror()
            }
            Row {
                x: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 20; height: 20
                    path: Icons.phone
                    color: Theme.accent
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Mirror whole screen")
                    weight: 500
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    role: "bodySmall"
                    muted: true
                    text: Mirror.muted ? qsTr("%1 · sound off").arg(page.qualityName)
                                       : qsTr("%1 · sound on").arg(page.qualityName)
                }
            }
            Row {
                anchors.right: parent.right
                anchors.rightMargin: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                LockChip {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: page.mirrorLocked && label.length > 0
                    feature: page.mirrorFeature
                }
                Keycap {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: !page.mirrorLocked
                    keys: "Ctrl M"
                }
            }
        }

        // ---- Apps in their own windows ----
        Item {
            id: appsHeader
            anchors.top: mirrorRow.bottom
            anchors.topMargin: 10
            width: parent.width
            height: 32

            SectionLabel {
                id: appsLabel
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Open an app in its own window")
            }
            Row {
                anchors.right: parent.right
                anchors.rightMargin: 2
                anchors.verticalCenter: parent.verticalCenter
                spacing: 10
                Field {
                    id: search
                    anchors.verticalCenter: parent.verticalCenter
                    visible: page.available
                    width: Math.max(140, Math.min(240, appsHeader.width - appsLabel.implicitWidth - lock.width - 40))
                    height: 28
                    iconPath: Icons.search
                    placeholder: qsTr("Search apps")
                    onAccepted: if (page.shown.length > 0) page.launch(page.shown[0])
                    Keys.onEscapePressed: search.text = ""
                    Keys.onDownPressed: grid.forceActiveFocus()
                    Accessible.name: qsTr("Search apps")
                }
                LockChip {
                    id: lock
                    anchors.verticalCenter: parent.verticalCenter
                    visible: label.length > 0
                    feature: page.feature
                }
            }
        }

        // Not available on this phone yet.
        Txt {
            anchors.top: appsHeader.bottom
            anchors.topMargin: 4
            x: Theme.rowPadding
            width: parent.width - 2 * Theme.rowPadding
            visible: !page.available
            wrapMode: Text.WordWrap
            muted: true
            text: page.feature.reason === "notOnThisDevice"
                ? qsTr("Not supported on this phone")
                : String(page.feature.reason || "").startsWith("android:")
                    ? qsTr("Needs Android %1 or newer").arg(page.feature.reason.slice(8))
                    : (page.feature.reason || qsTr("Set up Wireless debugging in G7 Link on the phone"))
        }

        // Loading, failed or empty.
        Row {
            anchors.top: appsHeader.bottom
            anchors.topMargin: 8
            x: Theme.rowPadding
            visible: page.available && (page.loadState !== "ready" || page.shown.length === 0)
            spacing: 10
            Spinner {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.loadState === "loading" || page.loadState === ""
                width: 16; height: 16
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, content.width - 160)
                muted: true
                text: page.loadState === "failed" ? Mirror.appsError
                    : page.loadState === "ready" ? (page.query.length > 0 ? qsTr("No apps match") : qsTr("No apps"))
                    : qsTr("Getting the phone's apps…")
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.loadState === "failed"
                text: qsTr("Try again")
                onClicked: Mirror.loadApps(page.deviceId)
            }
        }

        GridView {
            id: grid
            anchors.top: appsHeader.bottom
            anchors.topMargin: 2
            anchors.bottom: parent.bottom
            width: parent.width
            visible: page.available && page.loadState === "ready" && page.shown.length > 0
            clip: true
            model: page.shown
            readonly property int columns: Math.max(1, Math.floor(width / 220))
            cellWidth: Math.floor(width / columns)
            cellHeight: Theme.rowHeight + 2
            boundsBehavior: Flickable.StopAtBounds
            keyNavigationEnabled: true
            currentIndex: 0
            highlightFollowsCurrentItem: false
            cacheBuffer: 400
            Keys.onReturnPressed: page.launch(page.currentApp)
            Keys.onEnterPressed: page.launch(page.currentApp)

            header: Column {
                width: grid.width
                visible: page.query.length === 0 && page.recent.length > 0
                height: visible ? implicitHeight : 0
                Flow {
                    width: parent.width
                    Repeater {
                        model: page.recent
                        delegate: AppRow {
                            required property var modelData
                            app: modelData
                            width: grid.cellWidth
                            note: qsTr("recent")
                        }
                    }
                }
                SectionLabel {
                    width: parent.width
                    text: qsTr("All apps")
                }
            }

            delegate: AppRow {
                required property var modelData
                required property int index
                app: modelData
                width: grid.cellWidth
                current: grid.activeFocus && grid.currentIndex === index
            }
        }
    }

    // One app: its icon (or a tinted initial), its name, and "open" when it
    // has a window on this PC.
    component AppRow: ActionRow {
        id: appRow
        property var app: ({})
        property bool current: false
        property string note: ""
        readonly property bool isOpen: page.openPkgs[app.pkg] === true
        readonly property var tints: [Theme.tintTeal, Theme.tintBlue, Theme.tintCoral, Theme.tintPink,
                                      Theme.tintGreen, Theme.tintLavender, Theme.tintAmber]
        readonly property color appTint: {
            const label = app.label || ""
            let h = 0
            for (let i = 0; i < label.length; ++i)
                h = (h * 31 + label.charCodeAt(i)) % 7
            return tints[h]
        }
        height: Theme.rowHeight
        title: app.label || ""
        meta: isOpen ? qsTr("open") : note
        selected: current
        activeFocusOnTab: false
        accessibleName: isOpen ? qsTr("%1, open").arg(app.label) : (app.label || "")
        onClicked: page.launch(appRow.app)
        leading: Item {
            width: 26; height: 26
            Rectangle {
                anchors.fill: parent
                radius: 7
                visible: appIcon.status !== Image.Ready
                color: Theme.wash(appRow.appTint)
                Txt {
                    anchors.centerIn: parent
                    text: (appRow.app.label || "?").charAt(0).toUpperCase()
                    role: "label"
                    color: appRow.appTint
                }
            }
            Image {
                id: appIcon
                anchors.fill: parent
                source: appRow.app.icon || ""
                sourceSize: Qt.size(52, 52)
                asynchronous: true
                smooth: true
                mipmap: true
                visible: status === Image.Ready
            }
        }
    }
}
