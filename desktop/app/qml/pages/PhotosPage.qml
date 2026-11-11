// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Dialogs
import QtQuick.Window
import app.g7link

// Photos: the phone's gallery on the PC. Albums as tabs along the top, a
// virtualized grid of square thumbnails by day, multi-select, Save (to the
// default folder or a chosen one), Copy, Open in the default viewer, Delete,
// drag out, and an in-app viewer with arrow-key navigation.
Item {
    id: page
    property bool active: false
    property string deviceId
    property string deviceName

    readonly property var albums: { try { return JSON.parse(Photos.albums) } catch (e) { return [] } }
    readonly property var photosFeature: AppController.capsRevision >= 0 && page.deviceId.length > 0
        ? AppController.featureState(page.deviceId, "files.recent_photos") : ({})

    // Multi-select state: map of itemId -> true, and ordered array of selected IDs.
    property var selectedMap: ({})
    property var selectedIds: []
    // Delete asks once more: it reaches the phone's own gallery.
    property bool confirmDelete: false
    onSelectedIdsChanged: confirmDelete = false
    readonly property int selectedCount: selectedIds.length
    property bool selectMode: false
    property int lastClickedIndex: -1

    // In-app viewer index (-1 when closed).
    property int viewerIndex: -1
    readonly property var viewedItem: {
        if (Photos.revision < 0 || viewerIndex < 0 || viewerIndex >= Photos.count)
            return null
        try { return JSON.parse(Photos.itemAt(viewerIndex)) } catch (e) { return null }
    }

    // Whether the single selected item is a video (Copy is for still images).
    readonly property bool singleSelectedIsVideo: {
        if (selectedCount !== 1 || Photos.revision < 0) return false
        const id = selectedIds[0]
        return id.indexOf("video:") === 0
    }
    readonly property bool canCopySelection: selectedCount === 1 && !singleSelectedIsVideo && !Photos.saving

    readonly property string currentAlbumName: {
        if (Photos.album.length === 0) return qsTr("All photos")
        for (let i = 0; i < albums.length; i++) {
            if (albums[i].id === Photos.album) return albums[i].name
        }
        return qsTr("Album")
    }

    readonly property int totalPhotosCount: {
        let sum = 0
        for (let i = 0; i < albums.length; i++)
            sum += (albums[i].count || 0)
        return sum > 0 ? sum : Photos.count
    }
    // The default save folder's own name ("Pictures").
    readonly property string saveFolderName: {
        const f = Photos.saveFolder || ""
        const parts = f.split(/[\\/]/).filter((p) => p.length > 0)
        return parts.length > 0 ? parts[parts.length - 1] : f
    }

    function sameDay(a, b) {
        // By the calendar alone: a daylight-saving day has 25 hours.
        if (!a || !b) return false
        const da = new Date(a)
        const db = new Date(b)
        return da.getDate() === db.getDate()
            && da.getMonth() === db.getMonth()
            && da.getFullYear() === db.getFullYear()
    }

    function dayLabel(ms) {
        if (!ms || ms <= 0) return ""
        const date = new Date(ms)
        const today = new Date()
        const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1)
        if (date.toDateString() === today.toDateString()) return qsTr("Today")
        if (date.toDateString() === yesterday.toDateString()) return qsTr("Yesterday")
        const locale = Qt.locale()
        if (date.getFullYear() !== today.getFullYear())
            return date.toLocaleDateString(locale, Locale.LongFormat)
        const pattern = locale.dateFormat(Locale.LongFormat)
            .replace(/\s*yyyy\s*(?:'年'|年)\s*|[,\s]+yyyy\b|\byyyy[,\s]+/g, "")
        return date.toLocaleDateString(locale, pattern)
    }

    function shortDayLabel(ms) {
        if (!ms || ms <= 0) return ""
        const date = new Date(ms)
        const today = new Date()
        const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1)
        if (date.toDateString() === today.toDateString()) return qsTr("Today")
        if (date.toDateString() === yesterday.toDateString()) return qsTr("Yesterday")
        const locale = Qt.locale()
        let pattern = locale.dateFormat(Locale.LongFormat)
            .replace(/dddd[,\s]*|[,\s]*dddd/g, "")
            .replace(/MMMM/g, "MMM")
        if (date.getFullYear() === today.getFullYear())
            pattern = pattern.replace(/\s*yyyy\s*(?:'年'|年)\s*|[,\s]+yyyy\b|\byyyy[,\s]+/g, "")
        return date.toLocaleDateString(locale, pattern)
    }

    function formatVideoDuration(ms) {
        if (!ms || ms <= 0) return "0:00"
        const totalSec = Math.max(1, Math.round(ms / 1000))
        const h = Math.floor(totalSec / 3600)
        const m = Math.floor((totalSec % 3600) / 60)
        const s = totalSec % 60
        if (h > 0)
            return h + ":" + String(m).padStart(2, "0") + ":" + String(s).padStart(2, "0")
        return m + ":" + String(s).padStart(2, "0")
    }

    function formatBytes(bytes) {
        if (!bytes || bytes <= 0) return ""
        if (bytes < 1024) return qsTr("%1 B").arg(Math.round(bytes))
        const units = [qsTr("%1 KB"), qsTr("%1 MB"), qsTr("%1 GB"), qsTr("%1 TB")]
        let value = bytes / 1024
        let unit = 0
        while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++ }
        return units[unit].arg(unit === 0 ? Math.round(value) : (value < 10 ? Qt.locale().toString(value, "f", 1) : Math.round(value)))
    }

    function isSelected(id) {
        return !!selectedMap[id]
    }

    function toggleSelect(id, index, shiftHeld) {
        const next = Object.assign({}, selectedMap)
        if (shiftHeld && lastClickedIndex >= 0 && index >= 0 && Photos.count > 0) {
            const from = Math.min(lastClickedIndex, index)
            const to = Math.max(lastClickedIndex, index)
            for (let r = from; r <= to; r++) {
                const rowId = Photos.idAt(r)
                if (rowId.length > 0) next[rowId] = true
            }
        } else if (next[id]) {
            delete next[id]
        } else {
            next[id] = true
        }
        if (index >= 0) lastClickedIndex = index
        selectedMap = next
        selectedIds = Object.keys(next)
    }

    function selectAllLoaded() {
        const next = {}
        const n = Math.min(Photos.count, 500)
        for (let r = 0; r < n; r++) {
            const rowId = Photos.idAt(r)
            if (rowId.length > 0) next[rowId] = true
        }
        selectedMap = next
        selectedIds = Object.keys(next)
        selectMode = true
    }

    function clearSelection() {
        selectedMap = ({})
        selectedIds = []
        selectMode = false
        lastClickedIndex = -1
    }

    function openViewer(index) {
        if (index < 0 || index >= Photos.count) return
        viewerIndex = index
        try {
            const item = JSON.parse(Photos.itemAt(index))
            if (item && !item.isVideo) {
                Photos.ensureFull(item.id)
            }
        } catch (e) {}
        if (index + 6 >= Photos.count && Photos.more) {
            Photos.loadOlder()
        }
    }

    function stepViewer(delta) {
        const next = viewerIndex + delta
        if (next >= 0 && next < Photos.count) {
            openViewer(next)
        }
    }

    // The row of the one selected item (to open it), or -1.
    function selectedRow() {
        if (selectedCount !== 1)
            return lastClickedIndex
        const id = selectedIds[0]
        if (lastClickedIndex >= 0 && Photos.idAt(lastClickedIndex) === id)
            return lastClickedIndex
        for (let r = 0; r < Photos.count; r++) {
            if (Photos.idAt(r) === id)
                return r
        }
        return -1
    }
    // Space: opens the selected photo in the viewer, or closes the viewer.
    function toggleViewer() {
        if (viewerIndex >= 0) {
            viewerIndex = -1
            return
        }
        const row = selectedRow()
        openViewer(row >= 0 ? row : 0)
    }
    function saveSelection() {
        if (selectedCount === 0 || Photos.saving)
            return
        Photos.saveItems(selectedIds, "")
        clearSelection()
    }
    function saveSelectionTo() {
        if (selectedCount === 0 || Photos.saving)
            return
        saveToFolderPicker.pendingIds = selectedIds.slice()
        saveToFolderPicker.open()
    }
    function selectAlbum(id) {
        clearSelection()
        viewerIndex = -1
        Photos.selectAlbum(id)
    }

    function load() {
        if (active && deviceId.length > 0)
            Photos.open(deviceId)
    }

    onActiveChanged: if (active) load()
    Component.onCompleted: load()
    onDeviceIdChanged: {
        clearSelection()
        viewerIndex = -1
        load()
    }

    visible: active

    // The page's keys stay out of the way while something else, such as the
    // Actions list or a text field elsewhere, has the keyboard.
    readonly property bool keysHere: {
        const f = Window.activeFocusItem
        if (!f || f === Window.contentItem)
            return true
        for (let it = f; it; it = it.parent) {
            if (it === page)
                return true
        }
        return false
    }

    // ---- Footer and Actions (Ctrl B) ----
    readonly property string footerLabel: {
        const n = Photos.album.length === 0 ? totalPhotosCount : Photos.count
        const name = Photos.album.length === 0 ? qsTr("Photos") : currentAlbumName
        if (n <= 0)
            return name
        return Photos.more && Photos.album.length > 0
            ? qsTr("%1 · %2+").arg(name).arg(Qt.locale().toString(n))
            : qsTr("%1 · %2").arg(name).arg(Qt.locale().toString(n))
    }
    readonly property var primaryAction: Photos.count > 0
        ? { label: viewerIndex >= 0 ? qsTr("Close") : qsTr("Open"), keys: "Space", run: () => page.toggleViewer() }
        : null
    readonly property var screenActions: {
        const list = []
        const one = page.viewerIndex >= 0 ? page.viewedItem : null
        if (one) {
            const id = one.id
            if (!one.isVideo)
                list.push({ label: qsTr("Copy"), detail: one.name, icon: Icons.copy, tint: Theme.tintLavender, keys: "Ctrl C",
                            enabled: !Photos.saving, run: () => Photos.copyItem(id) })
            list.push({ label: one.isVideo ? qsTr("Play video") : qsTr("Open in Photos"), detail: one.name,
                        icon: one.isVideo ? Icons.play : Icons.openExternal, tint: Theme.tintBlue,
                        enabled: !Photos.saving, run: () => Photos.openItem(id) })
            list.push({ label: qsTr("Save to %1").arg(page.saveFolderName), icon: Icons.download, tint: Theme.accent,
                        enabled: !Photos.saving, run: () => Photos.saveItems([id], "") })
            list.push({ label: qsTr("Save to folder…"), icon: Icons.folder, tint: Theme.tintAmber, enabled: !Photos.saving,
                        run: () => { saveToFolderPicker.pendingIds = [id]; saveToFolderPicker.open() } })
            list.push({ label: qsTr("Previous photo"), icon: Icons.back, keys: "Left", enabled: page.viewerIndex > 0,
                        run: () => page.stepViewer(-1) })
            list.push({ label: qsTr("Next photo"), icon: Icons.chevronRight, keys: "Right",
                        enabled: page.viewerIndex + 1 < Photos.count, run: () => page.stepViewer(1) })
            list.push({ label: qsTr("Back to the photos"), icon: Icons.back, keys: "Esc", run: () => { page.viewerIndex = -1 } })
            return list
        }
        if (Photos.count > 0)
            list.push({ label: qsTr("Open"), icon: Icons.photo, tint: Theme.tintCoral, keys: "Space", run: () => page.toggleViewer() })
        if (page.selectedCount > 0) {
            const ids = page.selectedIds.slice()
            list.push({ label: qsTr("Save to %1").arg(page.saveFolderName),
                        detail: qsTr("%n selected", "", page.selectedCount), icon: Icons.download, tint: Theme.accent,
                        enabled: !Photos.saving, run: () => page.saveSelection() })
            list.push({ label: qsTr("Save to folder…"), icon: Icons.folder, tint: Theme.tintAmber, enabled: !Photos.saving,
                        run: () => page.saveSelectionTo() })
            if (page.selectedCount === 1) {
                if (!page.singleSelectedIsVideo)
                    list.push({ label: qsTr("Copy"), icon: Icons.copy, tint: Theme.tintLavender, keys: "Ctrl C",
                                enabled: !Photos.saving, run: () => Photos.copyItem(ids[0]) })
                list.push({ label: qsTr("Open in Photos"), icon: Icons.openExternal, tint: Theme.tintBlue,
                            enabled: !Photos.saving, run: () => Photos.openItem(ids[0]) })
            }
            list.push({ label: qsTr("Delete"), detail: qsTr("Moves to the phone's trash"), icon: Icons.trash, tint: Theme.tintRed,
                        enabled: !Photos.saving, run: () => { page.confirmDelete = true } })
            list.push({ label: qsTr("Clear selection"), icon: Icons.close, keys: "Esc", run: () => page.clearSelection() })
        }
        if (Photos.count > 0) {
            list.push({ label: Photos.count > 500 ? qsTr("Select first 500") : qsTr("Select all"), icon: Icons.check,
                        keys: "Ctrl A", run: () => page.selectAllLoaded() })
            if (!page.selectMode && page.selectedCount === 0)
                list.push({ label: qsTr("Select"), detail: qsTr("Click photos to pick them"), icon: Icons.check,
                            run: () => { page.selectMode = true } })
        }
        if (AppController.continuityCameraBusy) {
            list.push({ label: qsTr("Cancel camera"), detail: AppController.continuityCameraStatus, icon: Icons.close,
                        tint: Theme.tintRed, run: () => AppController.cancelContinuityCamera() })
        } else {
            list.push({ label: qsTr("Take photo with phone"), icon: Icons.camera, tint: Theme.tintBlue,
                        run: () => AppController.startContinuityCamera("photo") })
            list.push({ label: qsTr("Scan document with phone"), icon: Icons.clipboardList, tint: Theme.tintTeal,
                        run: () => AppController.startContinuityCamera("scan") })
        }
        if (Photos.count > 0)
            list.push({ label: qsTr("Default save folder"), detail: Photos.saveFolder || "", icon: Icons.folder,
                        tint: Theme.tintAmber, run: () => defaultFolderPicker.open() })
        list.push({ label: qsTr("Refresh"), icon: Icons.refresh, tint: Theme.tintBlue, enabled: Photos.status !== "loading",
                    run: () => Photos.refresh() })
        list.push({ label: qsTr("All photos"), detail: String(page.totalPhotosCount), icon: Icons.photo,
                    tint: Theme.tintCoral, checked: Photos.album === "", run: () => page.selectAlbum("") })
        for (const a of page.albums) {
            const albumId = a.id
            list.push({ label: a.name, detail: String(a.count || 0), icon: Icons.photo, tint: Theme.tintCoral,
                        checked: Photos.album === albumId, run: () => page.selectAlbum(albumId) })
        }
        return list
    }

    // Keyboard shortcuts for viewer navigation and selection.
    Shortcut {
        sequence: "Left"
        enabled: page.active && page.keysHere && page.viewerIndex > 0
        onActivated: page.stepViewer(-1)
    }
    Shortcut {
        sequence: "Right"
        enabled: page.active && page.keysHere && page.viewerIndex >= 0 && page.viewerIndex + 1 < Photos.count
        onActivated: page.stepViewer(1)
    }
    Shortcut {
        sequence: "Space"
        enabled: page.active && page.keysHere && Photos.count > 0
                 && (page.viewerIndex >= 0 || page.selectedCount === 1)
        onActivated: page.toggleViewer()
    }
    Shortcut {
        sequence: "Esc"
        enabled: page.active && page.keysHere && (page.viewerIndex >= 0 || page.selectedCount > 0 || page.selectMode || page.confirmDelete)
        onActivated: {
            if (page.viewerIndex >= 0)
                page.viewerIndex = -1
            else if (page.confirmDelete)
                page.confirmDelete = false
            else
                page.clearSelection()
        }
    }
    Shortcut {
        sequences: [StandardKey.SelectAll]
        enabled: page.active && page.keysHere && page.viewerIndex < 0 && Photos.count > 0
        onActivated: page.selectAllLoaded()
    }
    Shortcut {
        sequences: [StandardKey.Copy]
        enabled: page.active && page.keysHere && (
            (page.viewerIndex >= 0 && page.viewedItem !== null && !page.viewedItem.isVideo)
            || (page.selectedCount === 1 && !page.singleSelectedIsVideo)
        )
        // The same pick as `enabled`: a video in the viewer isn't copied over
        // the one image selected.
        onActivated: {
            if (page.viewerIndex >= 0 && page.viewedItem && !page.viewedItem.isVideo)
                Photos.copyItem(page.viewedItem.id)
            else if (page.selectedCount === 1 && !page.singleSelectedIsVideo)
                Photos.copyItem(page.selectedIds[0])
        }
    }

    FolderDialog {
        id: defaultFolderPicker
        title: qsTr("Default folder for saved photos")
        onAccepted: Photos.chooseSaveFolder(selectedFolder.toString())
    }

    FolderDialog {
        id: saveToFolderPicker
        property var pendingIds: []
        title: qsTr("Save to folder")
        onAccepted: {
            if (pendingIds.length > 0) {
                Photos.saveItems(pendingIds, selectedFolder.toString())
                if (page.viewerIndex < 0)
                    page.clearSelection()
            }
        }
    }

    readonly property bool phoneOffline: page.deviceId.length === 0
        || (Photos.status === "offline" && Photos.count === 0 && page.albums.length === 0)

    // ---- No phone paired, or phone offline with nothing cached ----
    Column {
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 360)
        spacing: 8
        visible: page.phoneOffline
        Icon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 24; height: 24
            path: Icons.photo
            color: Theme.tintCoral
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            muted: true
            text: page.deviceId.length === 0 ? qsTr("Pair a phone to see its photos")
                : qsTr("%1 isn't connected").arg(page.deviceName)
        }
    }

    // ---- Albums, selection and its actions ----
    Item {
        id: toolbar
        visible: !page.phoneOffline
        x: 16
        y: 12
        width: parent.width - 32
        height: 28

        Flickable {
            id: albumTabs
            anchors.left: parent.left
            anchors.right: toolbarRight.left
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            height: 26
            contentWidth: albumRow.implicitWidth
            interactive: contentWidth > width
            boundsBehavior: Flickable.StopAtBounds
            flickableDirection: Flickable.HorizontalFlick
            clip: true
            Row {
                id: albumRow
                spacing: 4
                TextAction {
                    text: qsTr("All")
                    selected: Photos.album === ""
                    Accessible.name: qsTr("All photos, %1").arg(page.totalPhotosCount)
                    onClicked: page.selectAlbum("")
                }
                Repeater {
                    model: page.albums
                    delegate: TextAction {
                        required property var modelData
                        text: modelData.name
                        selected: Photos.album === modelData.id
                        Accessible.name: qsTr("%1, %2").arg(modelData.name).arg(modelData.count || 0)
                        onClicked: page.selectAlbum(modelData.id)
                    }
                }
            }
        }

        Row {
            id: toolbarRight
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8

            // The phone's camera at work for this PC.
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                visible: AppController.continuityCameraBusy
                width: Math.min(implicitWidth, 260)
                role: "bodySmall"
                muted: true
                text: AppController.continuityCameraStatus
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                visible: AppController.continuityCameraBusy
                text: qsTr("Cancel")
                onClicked: AppController.cancelContinuityCamera()
            }

            Txt {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.selectedCount > 0 || page.selectMode
                role: "bodySmall"
                muted: true
                text: page.selectedCount > 0 ? qsTr("%n selected", "", page.selectedCount) : qsTr("Click photos to pick them")
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.selectedCount > 0 || page.selectMode
                text: qsTr("Clear")
                onClicked: page.clearSelection()
            }
            Button {
                anchors.verticalCenter: parent.verticalCenter
                visible: !page.confirmDelete
                variant: "outline"
                size: "sm"
                text: qsTr("Copy")
                enabled: page.canCopySelection
                onClicked: Photos.copyItem(page.selectedIds[0])
            }
            Button {
                anchors.verticalCenter: parent.verticalCenter
                visible: !page.confirmDelete
                size: "sm"
                busy: Photos.saving
                enabled: page.selectedCount > 0 && !Photos.saving
                text: page.saveFolderName.length > 0 ? qsTr("Save to %1").arg(page.saveFolderName) : qsTr("Save")
                onClicked: page.saveSelection()
            }

            // Delete, asked once more: it reaches the phone's own gallery.
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.confirmDelete
                role: "bodySmall"
                color: Theme.tintRed
                text: qsTr("Move %n to the phone's trash?", "", page.selectedCount)
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.confirmDelete
                text: qsTr("Cancel")
                onClicked: page.confirmDelete = false
            }
            Button {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.confirmDelete
                variant: "outline"
                size: "sm"
                iconPath: Icons.trash
                text: qsTr("Delete")
                enabled: page.selectedCount > 0 && !Photos.saving
                onClicked: {
                    Photos.deleteItems(page.selectedIds)
                    page.clearSelection()
                }
            }
        }
    }

    // ---- The photos ----
    Item {
        id: gridContainer
        visible: !page.phoneOffline
        anchors.top: toolbar.bottom
        anchors.topMargin: 8
        anchors.bottom: parent.bottom
        x: 16
        width: parent.width - 32

        Row {
            anchors.left: parent.left
            anchors.leftMargin: Theme.rowPadding
            y: 40
            visible: Photos.status === "loading" && Photos.count === 0
            spacing: 10
            Spinner { width: 16; height: 16; anchors.verticalCenter: parent.verticalCenter }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                muted: true
                text: qsTr("Getting photos…")
            }
        }

        // Off, not allowed, offline or failed.
        Column {
            anchors.centerIn: parent
            width: Math.min(parent.width - 48, 360)
            spacing: 8
            visible: Photos.count === 0 && Photos.status !== "loading" && Photos.status !== "ready"
            Icon {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 24; height: 24
                path: Icons.photo
                color: Theme.tintCoral
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                text: Photos.status === "off" ? qsTr("Photos are off for %1").arg(page.deviceName)
                    : Photos.status === "unsupported" ? qsTr("Allow photos on %1").arg(page.deviceName)
                    : Photos.status === "offline" ? qsTr("%1 isn't connected").arg(page.deviceName)
                    : qsTr("Couldn't load photos")
            }
            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                role: "bodySmall"
                muted: true
                visible: Photos.status === "off" || Photos.status === "unsupported"
                text: Photos.status === "off" ? qsTr("Turn on Photos in Settings")
                    : qsTr("Grant the photos permission on the phone")
            }
            LockChip {
                anchors.horizontalCenter: parent.horizontalCenter
                visible: label.length > 0
                feature: page.photosFeature
            }
            TextAction {
                anchors.horizontalCenter: parent.horizontalCenter
                visible: Photos.status === "failed"
                text: qsTr("Try again")
                onClicked: Photos.refresh()
            }
        }

        Txt {
            anchors.centerIn: parent
            visible: Photos.status === "ready" && Photos.count === 0
            muted: true
            text: qsTr("No photos or videos")
        }

        // The day at the top of the grid.
        SectionLabel {
            id: dayHeader
            width: parent.width
            visible: Photos.count > 0
            text: grid.stickyDay
        }

        // Virtualized thumbnail grid; tiles that start a new day say so.
        GridView {
            id: grid
            anchors.top: dayHeader.bottom
            anchors.bottom: parent.bottom
            width: parent.width
            bottomMargin: 16
            visible: Photos.count > 0
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            cacheBuffer: Math.max(0, height)
            // Auto-fill: as many ~130 px columns as fit, 6 px apart.
            readonly property int columns: Math.max(2, Math.floor((width + 6) / 136))
            cellWidth: Math.floor(width / columns)
            cellHeight: cellWidth
            model: Photos

            onAtYEndChanged: {
                if (atYEnd && Photos.more && count > 0)
                    Photos.loadOlder()
            }

            readonly property int topVisibleRow: Math.max(
                0,
                Math.floor(Math.max(0, contentY + 20) / Math.max(1, cellHeight)) * columns
            )
            readonly property string stickyDay: {
                if (Photos.revision < 0 || Photos.count === 0) return ""
                const row = Math.min(topVisibleRow, Photos.count - 1)
                const ms = Photos.dateAt(row)
                return ms > 0 ? page.dayLabel(ms) : ""
            }

            delegate: PhotoTile {
                width: grid.cellWidth
                height: grid.cellHeight
            }

            footer: Item {
                width: grid.width
                height: Photos.loadingOlder ? 48 : 0
                Spinner {
                    anchors.centerIn: parent
                    visible: Photos.loadingOlder
                    width: 16; height: 16
                }
            }
        }
    }

    // ---- The viewer ----
    Rectangle {
        id: viewer
        readonly property bool opened: page.viewerIndex >= 0 && page.viewedItem !== null
        anchors.fill: parent
        visible: opacity > 0
        opacity: opened ? 1.0 : 0.0
        Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast; easing.type: Easing.OutCubic } }
        color: Theme.surface
        z: 10

        // Absorb background clicks.
        TapHandler { onTapped: {} }

        Item {
            id: viewerBar
            width: parent.width
            height: 48

            IconButton {
                id: closeViewerBtn
                anchors.left: parent.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                iconPath: Icons.back
                label: qsTr("Back to the photos")
                onClicked: page.viewerIndex = -1
            }
            Txt {
                id: viewerName
                anchors.left: closeViewerBtn.right
                anchors.leftMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, (viewerActions.x - x) * 0.5)
                weight: 500
                elide: Text.ElideMiddle
                text: page.viewedItem ? page.viewedItem.name : ""
            }
            Txt {
                anchors.left: viewerName.right
                anchors.leftMargin: 10
                anchors.right: viewerActions.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                role: "bodySmall"
                muted: true
                text: {
                    if (!page.viewedItem) return ""
                    const parts = []
                    parts.push((page.viewerIndex + 1) + " / " + Photos.count)
                    if (page.viewedItem.date > 0) {
                        const d = new Date(page.viewedItem.date)
                        parts.push(page.dayLabel(page.viewedItem.date) + " " + d.toLocaleTimeString(Qt.locale(), Locale.ShortFormat))
                    }
                    if (page.viewedItem.width > 0 && page.viewedItem.height > 0)
                        parts.push(page.viewedItem.width + " × " + page.viewedItem.height)
                    if (page.viewedItem.size > 0)
                        parts.push(page.formatBytes(page.viewedItem.size))
                    return parts.join(" · ")
                }
            }

            Row {
                id: viewerActions
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8

                Button {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: page.viewedItem !== null && !page.viewedItem.isVideo
                    variant: "outline"
                    size: "sm"
                    text: qsTr("Copy")
                    enabled: !Photos.saving
                    onClicked: if (page.viewedItem) Photos.copyItem(page.viewedItem.id)
                }
                Button {
                    anchors.verticalCenter: parent.verticalCenter
                    variant: "outline"
                    size: "sm"
                    text: page.viewedItem && page.viewedItem.isVideo ? qsTr("Play video") : qsTr("Open in Photos")
                    enabled: !Photos.saving
                    onClicked: if (page.viewedItem) Photos.openItem(page.viewedItem.id)
                }
                Button {
                    anchors.verticalCenter: parent.verticalCenter
                    size: "sm"
                    busy: Photos.saving
                    enabled: !Photos.saving
                    text: page.saveFolderName.length > 0 ? qsTr("Save to %1").arg(page.saveFolderName) : qsTr("Save")
                    onClicked: if (page.viewedItem) Photos.saveItems([page.viewedItem.id], "")
                }
                IconButton {
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: Icons.folder
                    label: qsTr("Save to folder…")
                    enabled: !Photos.saving
                    onClicked: {
                        if (page.viewedItem) {
                            saveToFolderPicker.pendingIds = [page.viewedItem.id]
                            saveToFolderPicker.open()
                        }
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

        Item {
            anchors.top: viewerBar.bottom
            anchors.bottom: parent.bottom
            width: parent.width

            Image {
                id: stageImage
                anchors.fill: parent
                anchors.margins: 24
                asynchronous: true
                fillMode: Image.PreserveAspectFit
                smooth: true
                mipmap: true
                source: {
                    if (!page.viewedItem) return ""
                    if (page.viewedItem.fullUrl && page.viewedItem.fullUrl.length > 0)
                        return page.viewedItem.fullUrl
                    return page.viewedItem.thumb || ""
                }
            }

            // Full resolution still on its way.
            Rectangle {
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 16
                anchors.horizontalCenter: parent.horizontalCenter
                visible: page.viewedItem !== null
                    && !page.viewedItem.isVideo
                    && (!page.viewedItem.fullUrl || page.viewedItem.fullUrl.length === 0)
                    && Photos.busyItem === page.viewedItem.id
                height: 28
                width: loadingRow.implicitWidth + 20
                radius: Theme.rowRadius
                color: Theme.raised
                Row {
                    id: loadingRow
                    anchors.centerIn: parent
                    spacing: 8
                    Spinner { width: 14; height: 14; anchors.verticalCenter: parent.verticalCenter }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        role: "bodySmall"
                        muted: true
                        text: qsTr("Loading full resolution…")
                    }
                }
            }

            // Play a video in the default player.
            Rectangle {
                id: playOverlayBtn
                anchors.centerIn: parent
                visible: page.viewedItem !== null && !!page.viewedItem.isVideo
                width: 64; height: 64
                radius: 32
                color: Qt.rgba(0, 0, 0, 0.62)
                activeFocusOnTab: true
                Accessible.role: Accessible.Button
                Accessible.name: qsTr("Play video")
                Accessible.onPressAction: if (page.viewedItem) Photos.openItem(page.viewedItem.id)
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                        if (page.viewedItem)
                            Photos.openItem(page.viewedItem.id)
                        event.accepted = true
                    }
                }
                border.width: Theme.focusVisible(playOverlayBtn) ? 2 : 0
                border.color: Theme.focusRing
                Icon {
                    anchors.centerIn: parent
                    width: 26; height: 26
                    path: Icons.play
                    color: "#ffffff"
                }
                HoverHandler { cursorShape: Qt.PointingHandCursor }
                TapHandler {
                    onTapped: if (page.viewedItem) Photos.openItem(page.viewedItem.id)
                }
            }

            IconButton {
                anchors.left: parent.left
                anchors.leftMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                tonal: true
                visible: page.viewerIndex > 0
                iconPath: Icons.back
                label: qsTr("Previous photo")
                onClicked: page.stepViewer(-1)
            }
            IconButton {
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                tonal: true
                visible: page.viewerIndex + 1 < Photos.count
                iconPath: Icons.chevronRight
                label: qsTr("Next photo")
                onClicked: page.stepViewer(1)
            }
        }
    }

    // ---- One thumbnail ----
    component PhotoTile: Item {
        id: tile
        required property int index
        required property string itemId
        required property string name
        required property real date
        required property real prevDate
        required property real size
        required property int itemWidth
        required property int itemHeight
        required property int duration
        required property bool isVideo
        required property string album
        required property string thumb
        required property string fullUrl

        property string requestedThumbId: ""
        readonly property bool selected: page.selectedCount >= 0 && page.isSelected(itemId)
        readonly property bool firstOfDay: index === 0 || !page.sameDay(date, prevDate)

        function activateTile() {
            if (page.selectMode || page.selectedCount > 0)
                page.toggleSelect(tile.itemId, tile.index, false)
            else
                page.openViewer(tile.index)
        }

        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: tile.name
        Accessible.onPressAction: tile.activateTile()
        Keys.onReturnPressed: tile.activateTile()
        Keys.onEnterPressed: tile.activateTile()
        // Space picks the tile (once one is picked, Space opens it instead);
        // Ctrl+Space always picks.
        Keys.onSpacePressed: page.toggleSelect(tile.itemId, tile.index, false)

        function checkThumb() {
            if (thumb.length === 0 && itemId.length > 0) {
                if (requestedThumbId !== itemId) {
                    if (requestedThumbId.length > 0)
                        Photos.dropThumb(requestedThumbId)
                    requestedThumbId = itemId
                    Photos.needThumb(itemId)
                }
            } else if (thumb.length > 0) {
                requestedThumbId = ""
            }
        }

        Component.onCompleted: checkThumb()
        onItemIdChanged: checkThumb()
        onThumbChanged: if (thumb.length > 0) requestedThumbId = ""
        Component.onDestruction: {
            if (requestedThumbId.length > 0 && thumb.length === 0)
                Photos.dropThumb(requestedThumbId)
        }

        Item {
            id: cellCard
            anchors.fill: parent
            anchors.margins: 3

            Rectangle {
                anchors.fill: parent
                radius: Theme.rowRadius
                color: Theme.raised
                Icon {
                    anchors.centerIn: parent
                    width: 22; height: 22
                    path: tile.isVideo ? Icons.video : Icons.photo
                    color: Theme.tintCoral
                    visible: thumbImg.status !== Image.Ready
                }
            }

            Image {
                id: thumbImg
                anchors.fill: parent
                asynchronous: true
                fillMode: Image.PreserveAspectCrop
                sourceSize: Qt.size(256, 256)
                source: tile.thumb
                opacity: status === Image.Ready ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
            }

            // Hover darkens a little.
            Rectangle {
                anchors.fill: parent
                radius: Theme.rowRadius
                color: tileHover.hovered && !tile.selected ? Qt.rgba(0, 0, 0, 0.14) : "transparent"
            }

            // Rounds the corners: a ring in the page's ground just outside
            // the tile, its inner edge an 8 px radius.
            Rectangle {
                anchors.fill: parent
                anchors.margins: -3
                radius: Theme.rowRadius + 3
                color: "transparent"
                border.width: 3
                border.color: Theme.surface
            }

            // Selected, or keyboard focus: a 2 px outline.
            Rectangle {
                anchors.fill: parent
                radius: Theme.rowRadius
                color: "transparent"
                border.width: tile.selected || Theme.focusVisible(tile) ? 2 : 0
                border.color: Theme.accent
            }

            // The first photo of each day says which day.
            Rectangle {
                anchors.top: parent.top
                anchors.topMargin: 6
                anchors.left: parent.left
                anchors.leftMargin: 6
                visible: tile.firstOfDay && tile.index > 0 && tile.date > 0
                height: 18
                width: Math.min(parent.width - 36, dayBadgeTxt.implicitWidth + 12)
                radius: 5
                color: Qt.rgba(0, 0, 0, 0.62)
                Txt {
                    id: dayBadgeTxt
                    anchors.centerIn: parent
                    width: parent.width - 10
                    horizontalAlignment: Text.AlignHCenter
                    role: "caption"
                    color: "#ffffff"
                    text: (tile.firstOfDay && tile.index > 0 && tile.date > 0)
                        ? page.shortDayLabel(tile.date) : ""
                }
            }

            // A video: a play mark and how long it runs.
            Rectangle {
                anchors.centerIn: parent
                visible: tile.isVideo
                width: 30; height: 30
                radius: 15
                color: Qt.rgba(0, 0, 0, 0.62)
                Icon {
                    anchors.centerIn: parent
                    width: 14; height: 14
                    path: Icons.play
                    color: "#ffffff"
                }
            }
            Rectangle {
                anchors.right: parent.right
                anchors.rightMargin: 6
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 6
                visible: tile.isVideo
                height: 18
                width: durTxt.implicitWidth + 10
                radius: 5
                color: Qt.rgba(0, 0, 0, 0.62)
                Txt {
                    id: durTxt
                    anchors.centerIn: parent
                    role: "caption"
                    color: "#ffffff"
                    text: tile.isVideo ? page.formatVideoDuration(tile.duration) : ""
                }
            }

            // Select: a lavender check when picked, a ring on hover.
            Rectangle {
                id: checkCircle
                z: 2
                anchors.top: parent.top
                anchors.topMargin: 6
                anchors.right: parent.right
                anchors.rightMargin: 6
                width: 18; height: 18
                radius: 9
                visible: tile.selected || tileHover.hovered || page.selectMode || page.selectedCount > 0
                color: tile.selected ? Theme.accent : Qt.rgba(0, 0, 0, 0.45)
                border.width: tile.selected ? 0 : 1.5
                border.color: "#ffffff"
                Accessible.role: Accessible.CheckBox
                Accessible.name: qsTr("Select photo")
                Accessible.checkable: true
                Accessible.checked: tile.selected
                Accessible.onPressAction: page.toggleSelect(tile.itemId, tile.index, false)
                Accessible.onToggleAction: page.toggleSelect(tile.itemId, tile.index, false)
                Icon {
                    anchors.centerIn: parent
                    visible: tile.selected
                    width: 12; height: 12
                    path: Icons.check
                    color: Theme.accentContent
                }
                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    onClicked: (mouse) => page.toggleSelect(tile.itemId, tile.index,
                                                            (mouse.modifiers & Qt.ShiftModifier) !== 0)
                }
            }

            HoverHandler { id: tileHover; cursorShape: Qt.PointingHandCursor }
            // Under the selection circle (its z is higher). A MouseArea, as
            // a TapHandler's event point doesn't carry the keyboard modifiers.
            // Dragging a tile out hands the photo to another app.
            MouseArea {
                id: mouse
                anchors.fill: parent
                property point pressPos: Qt.point(0, 0)
                property bool dragStarted: false
                onPressed: (ev) => {
                    pressPos = Qt.point(ev.x, ev.y)
                    dragStarted = false
                    if (!tile.isVideo && tile.fullUrl.length === 0)
                        Photos.ensureFull(tile.itemId)
                }
                onPositionChanged: (ev) => {
                    if (pressed && !dragStarted
                        && Math.hypot(ev.x - pressPos.x, ev.y - pressPos.y) > Qt.styleHints.startDragDistance) {
                        dragStarted = true
                        Photos.startDrag(tile.itemId)
                    }
                }
                onClicked: (ev) => {
                    if (dragStarted)
                        return
                    const mods = ev.modifiers
                    const shift = (mods & Qt.ShiftModifier) !== 0
                    const ctrl = (mods & Qt.ControlModifier) !== 0
                    if (page.selectMode || page.selectedCount > 0 || shift || ctrl) {
                        page.toggleSelect(tile.itemId, tile.index, shift)
                    } else {
                        page.openViewer(tile.index)
                    }
                }
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 6
        radius: Theme.radiusLg
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: opacity > 0
        opacity: AppController.highlightTarget === "photos:main" ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }
}
