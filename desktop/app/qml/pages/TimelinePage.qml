// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Timeline: searchable, paged local history of everything that moved between
// this PC and paired devices (files, folders, clips, links, saved photos,
// voice recordings, and screen mirroring / webcam sessions). One row each,
// by day; the selected row's actions are in the footer and in Actions.
Item {
    id: page
    property bool active: true
    property bool confirmClearAll: false
    // The selected entry, as its row's roles (null when none).
    property var current: null

    visible: active

    readonly property var kindOptions: [
        { key: "", label: qsTr("All") },
        { key: "file", label: qsTr("Files") },
        { key: "photo", label: qsTr("Photos") },
        { key: "clip", label: qsTr("Clipboard") },
        { key: "link", label: qsTr("Links") },
        { key: "recording", label: qsTr("Recordings") },
        { key: "session", label: qsTr("Sessions") }
    ]

    readonly property string selectedDeviceName: {
        if (TimelineModel.deviceFilter.length === 0)
            return qsTr("All devices")
        // Filtered by a device that's been unpaired since: not "all".
        const row = DeviceList.count > 0 ? DeviceList.rowOf(TimelineModel.deviceFilter) : -1
        return row >= 0 ? DeviceList.deviceNameAt(row) : qsTr("A removed device")
    }

    // How long the timeline keeps things (from the data retention summary).
    readonly property int retentionDays: {
        const raw = AppController.dataRetentionSummary
        if (!raw || raw.length === 0)
            return 0
        try {
            return JSON.parse(raw).timelineRetentionDays || 0
        } catch (e) {
            return 0
        }
    }
    onActiveChanged: if (active) AppController.refreshDataRetention()
    Component.onCompleted: AppController.refreshDataRetention()

    // ---- Footer and Actions (Ctrl B) ----
    readonly property string footerLabel: retentionDays > 0
        ? qsTr("Timeline · kept %n day(s)", "", retentionDays) : qsTr("Timeline")
    readonly property var primaryAction: current && current.canOpen
        ? { label: qsTr("Open"), keys: "Enter", run: () => page.openCurrent() } : null
    readonly property var secondaryAction: current && current.canShowInFolder
        ? { label: qsTr("Show in folder"), keys: "Ctrl O", run: () => page.revealCurrent() } : null
    readonly property var screenActions: {
        const list = []
        const c = page.current
        if (c) {
            const id = c.entryId
            if (c.canOpen)
                list.push({ label: qsTr("Open"), detail: c.title, icon: c.kind === "link" ? Icons.openExternal : Icons.folder,
                            tint: Theme.accent, keys: "Enter", run: () => TimelineModel.openEntry(id) })
            if (c.canShowInFolder)
                list.push({ label: qsTr("Show in folder"), detail: c.title, icon: Icons.folder, tint: Theme.tintAmber,
                            keys: "Ctrl O", run: () => TimelineModel.revealEntry(id) })
            if (c.canCopy)
                list.push({ label: qsTr("Copy"), detail: c.title, icon: Icons.copy, tint: Theme.tintLavender,
                            run: () => TimelineModel.copyEntry(id) })
            if (c.canSendAgain)
                list.push({ label: qsTr("Send again"), detail: c.title, icon: Icons.send, tint: Theme.tintAmber,
                            run: () => TimelineModel.sendAgain(id, "") })
            list.push({ label: qsTr("Remove from timeline"), detail: c.title, icon: Icons.close, tint: Theme.tintRed,
                        keys: "Del", run: () => page.removeEntry(id) })
        }
        list.push({ label: qsTr("Search timeline"), icon: Icons.search, tint: Theme.surfaceContentVariant, keys: "Ctrl F",
                    run: () => searchField.forceActiveFocus() })
        for (const k of page.kindOptions) {
            const key = k.key
            list.push({ label: qsTr("Show: %1").arg(k.label), icon: page.iconForKind(key), tint: page.tintForKind(key),
                        checked: TimelineModel.kindFilter === key, run: () => { TimelineModel.kindFilter = key } })
        }
        if (DeviceList.count > 1) {
            list.push({ label: qsTr("Show: All devices"), icon: Icons.phone, tint: Theme.surfaceContentVariant,
                        checked: TimelineModel.deviceFilter === "", run: () => { TimelineModel.deviceFilter = "" } })
            for (let i = 0; i < DeviceList.count; ++i) {
                const id = DeviceList.deviceIdAt(i)
                list.push({ label: qsTr("Show: %1").arg(DeviceList.deviceNameAt(i)), icon: Icons.phone,
                            tint: Theme.surfaceContentVariant, checked: TimelineModel.deviceFilter === id,
                            run: () => { TimelineModel.deviceFilter = id } })
            }
        }
        if (TimelineModel.hasMore)
            list.push({ label: qsTr("Load more"), icon: Icons.download, tint: Theme.tintBlue, run: () => TimelineModel.loadMore() })
        if (TimelineModel.count > 0)
            list.push({ label: qsTr("Clear timeline"), detail: qsTr("Saved files stay on disk"), icon: Icons.trash,
                        tint: Theme.tintRed, run: () => { page.confirmClearAll = true } })
        return list
    }

    function openCurrent() {
        if (current && current.canOpen)
            TimelineModel.openEntry(current.entryId)
    }
    function revealCurrent() {
        if (current && current.canShowInFolder)
            TimelineModel.revealEntry(current.entryId)
    }
    function removeEntry(id) {
        if (current && current.entryId === id)
            current = null
        TimelineModel.removeEntry(id)
    }

    Shortcut {
        sequence: "Esc"
        enabled: page.active && !menu.opened
                 && (page.confirmClearAll || listView.activeFocus || searchField.input.activeFocus)
        onActivated: {
            if (page.confirmClearAll) {
                page.confirmClearAll = false
            } else if (searchField.text.length > 0) {
                searchField.text = ""
                TimelineModel.searchQuery = ""
            } else if (page.current !== null) {
                page.current = null
                listView.currentIndex = -1
            } else {
                searchField.forceActiveFocus()
            }
        }
    }
    Shortcut {
        sequences: ["Return", "Enter"]
        enabled: page.active && listView.activeFocus && page.current !== null && page.current.canOpen
        onActivated: page.openCurrent()
    }
    Shortcut {
        sequence: "Ctrl+O"
        enabled: page.active && page.current !== null && page.current.canShowInFolder
        onActivated: page.revealCurrent()
    }
    Shortcut {
        sequence: StandardKey.Delete
        enabled: page.active && listView.activeFocus && page.current !== null
        onActivated: page.removeEntry(page.current.entryId)
    }
    Shortcut {
        sequences: [StandardKey.Find]
        enabled: page.active
        onActivated: searchField.forceActiveFocus()
    }

    Connections {
        target: TimelineModel
        function onHasAnyChanged() {
            if (!TimelineModel.hasAny) {
                page.confirmClearAll = false
                page.current = null
                if (searchField.text.length > 0)
                    searchField.text = ""
                if (TimelineModel.searchQuery.length > 0)
                    TimelineModel.searchQuery = ""
                if (TimelineModel.kindFilter.length > 0)
                    TimelineModel.kindFilter = ""
                if (TimelineModel.deviceFilter.length > 0)
                    TimelineModel.deviceFilter = ""
            }
        }
        function onCountChanged() {
            if (TimelineModel.count === 0)
                page.current = null
        }
        function onKindFilterChanged() { page.current = null; listView.currentIndex = -1 }
        function onDeviceFilterChanged() { page.current = null; listView.currentIndex = -1 }
        function onSearchQueryChanged() { page.current = null; listView.currentIndex = -1 }
    }

    Connections {
        target: DeviceList
        function onCountChanged() {
            if (DeviceList.count <= 1 && TimelineModel.deviceFilter.length > 0)
                TimelineModel.deviceFilter = ""
        }
    }

    function dayKey(unixSecs) {
        if (!unixSecs || unixSecs <= 0)
            return ""
        return new Date(unixSecs * 1000).toDateString()
    }

    function dayLabel(unixSecs) {
        const date = new Date(unixSecs * 1000)
        const today = new Date()
        const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1)
        if (date.toDateString() === today.toDateString())
            return qsTr("Today")
        if (date.toDateString() === yesterday.toDateString())
            return qsTr("Yesterday")
        const locale = Qt.locale()
        if (date.getFullYear() !== today.getFullYear())
            return date.toLocaleDateString(locale, Locale.LongFormat)
        const pattern = locale.dateFormat(Locale.LongFormat)
            .replace(/\s*yyyy\s*(?:'年'|年)\s*|[,\s]+yyyy\b|\byyyy[,\s]+/g, "")
        return date.toLocaleDateString(locale, pattern)
    }

    function timeOfDay(unixSecs) {
        return new Date(unixSecs * 1000).toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
    }

    function iconForKind(kind) {
        switch (kind) {
        case "file": return Icons.folder
        case "clip": return Icons.clipboard
        case "link": return Icons.globe
        case "photo": return Icons.photo
        case "recording": return Icons.mic
        case "session": return Icons.mirror
        default: return Icons.history
        }
    }

    function tintForKind(kind) {
        switch (kind) {
        case "file": return Theme.tintAmber
        case "clip": return Theme.tintLavender
        case "link": return Theme.tintBlue
        case "photo": return Theme.tintCoral
        case "recording": return Theme.tintRed
        case "session": return Theme.tintPink
        default: return Theme.surfaceContentVariant
        }
    }

    function iconForRow(kind, title) {
        if (kind === "file") {
            const t = (title || "").toLowerCase()
            if (/\.(jpg|jpeg|png|webp|gif|bmp|heic|svg)$/.test(t)) return Icons.photo
            if (/\.(mp4|mkv|mov|webm|avi)$/.test(t)) return Icons.video
            if (/\.(mp3|m4a|wav|flac|ogg|opus|aac)$/.test(t)) return Icons.music
        }
        return iconForKind(kind)
    }

    // "Phone → PC" / "PC → Phone", naming the device when there's more than one.
    function directionText(incoming, deviceName) {
        const other = DeviceList.count > 1 && deviceName.length > 0 ? deviceName : qsTr("Phone")
        return incoming ? qsTr("%1 → PC").arg(other) : qsTr("PC → %1").arg(other)
    }

    function rowActions(row) {
        const id = row.entryId
        const list = []
        if (row.canOpen)
            list.push({ label: qsTr("Open"), icon: row.kind === "link" ? Icons.openExternal : Icons.folder,
                        tint: Theme.accent, keys: "Enter", run: () => TimelineModel.openEntry(id) })
        if (row.canShowInFolder)
            list.push({ label: qsTr("Show in folder"), icon: Icons.folder, tint: Theme.tintAmber, keys: "Ctrl O",
                        run: () => TimelineModel.revealEntry(id) })
        if (row.canCopy)
            list.push({ label: qsTr("Copy"), icon: Icons.copy, tint: Theme.tintLavender, run: () => TimelineModel.copyEntry(id) })
        if (row.canSendAgain)
            list.push({ label: qsTr("Send again"), icon: Icons.send, tint: Theme.tintAmber,
                        run: () => TimelineModel.sendAgain(id, "") })
        list.push({ label: qsTr("Remove from timeline"), icon: Icons.close, tint: Theme.tintRed, keys: "Del",
                    run: () => page.removeEntry(id) })
        return list
    }

    // ---- Filters, device, search ----
    Item {
        id: bar
        visible: TimelineModel.hasAny
        x: 8
        y: 8
        width: parent.width - 16
        height: 32

        Flickable {
            id: kindTabs
            anchors.left: parent.left
            anchors.right: barRight.left
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            height: 26
            contentWidth: kindRow.implicitWidth
            interactive: contentWidth > width
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            Row {
                id: kindRow
                x: 2
                spacing: 4
                Repeater {
                    model: page.kindOptions
                    delegate: TextAction {
                        required property var modelData
                        text: modelData.label
                        selected: TimelineModel.kindFilter === modelData.key
                        onClicked: TimelineModel.kindFilter = modelData.key
                    }
                }
            }
        }
        Row {
            id: barRight
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            TextAction {
                id: deviceButton
                anchors.verticalCenter: parent.verticalCenter
                visible: DeviceList.count > 1
                text: page.selectedDeviceName
                iconPath: Icons.phone
                iconColor: Theme.surfaceContentVariant
                selected: TimelineModel.deviceFilter.length > 0
                Accessible.role: Accessible.ComboBox
                Accessible.name: qsTr("Device filter: %1").arg(page.selectedDeviceName)
                onClicked: {
                    const list = [{ label: qsTr("All devices"), icon: Icons.phone, tint: Theme.surfaceContentVariant,
                                    checked: TimelineModel.deviceFilter === "",
                                    run: () => { TimelineModel.deviceFilter = "" } }]
                    for (let i = 0; i < DeviceList.count; ++i) {
                        const id = DeviceList.deviceIdAt(i)
                        list.push({ label: DeviceList.deviceNameAt(i), icon: Icons.phone, tint: Theme.surfaceContentVariant,
                                    checked: TimelineModel.deviceFilter === id,
                                    run: () => { TimelineModel.deviceFilter = id } })
                    }
                    menu.actions = list
                    const p = deviceButton.mapToItem(page, deviceButton.width, deviceButton.height)
                    menu.openAt(p.x, p.y, false, true)
                }
            }
            Field {
                id: searchField
                anchors.verticalCenter: parent.verticalCenter
                width: Math.max(140, Math.min(240, bar.width * 0.3))
                height: 28
                iconPath: Icons.search
                placeholder: qsTr("Search timeline")
                text: TimelineModel.searchQuery
                Accessible.name: qsTr("Search timeline")
                Keys.onDownPressed: listView.forceActiveFocus()
                Connections {
                    target: searchField.input
                    function onTextEdited() { TimelineModel.searchQuery = searchField.text }
                }
            }
        }
    }

    // Clear all, asked once more.
    Item {
        id: confirmBar
        anchors.top: bar.bottom
        anchors.topMargin: 4
        x: 8
        width: parent.width - 16
        height: visible ? Theme.rowHeight : 0
        visible: page.confirmClearAll && TimelineModel.count > 0
        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: Theme.raised
        }
        Icon {
            id: confirmIcon
            x: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            width: 18; height: 18
            path: Icons.trash
            color: Theme.tintRed
        }
        Txt {
            anchors.left: confirmIcon.right
            anchors.leftMargin: 10
            anchors.right: confirmButtons.left
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Clear the timeline? Saved files stay on disk.")
        }
        Row {
            id: confirmButtons
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Cancel")
                onClicked: page.confirmClearAll = false
            }
            Button {
                anchors.verticalCenter: parent.verticalCenter
                size: "sm"
                text: qsTr("Clear timeline")
                onClicked: {
                    page.confirmClearAll = false
                    page.current = null
                    TimelineModel.clearAll()
                }
            }
        }
    }

    // ---- Nothing yet, or nothing matches ----
    Column {
        anchors.centerIn: parent
        width: Math.min(360, parent.width - 48)
        spacing: 8
        visible: TimelineModel.count === 0
        Icon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 24; height: 24
            path: Icons.history
            color: Theme.tintLavender
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            muted: true
            text: TimelineModel.hasAny ? qsTr("Nothing matches") : qsTr("Files, clips, links and photos you share show up here")
        }
        TextAction {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: TimelineModel.hasAny
            text: qsTr("Clear filters")
            onClicked: {
                searchField.text = ""
                TimelineModel.searchQuery = ""
                TimelineModel.kindFilter = ""
                TimelineModel.deviceFilter = ""
            }
        }
    }

    // ---- The entries, by day ----
    ListView {
        id: listView
        anchors.top: confirmBar.visible ? confirmBar.bottom : bar.bottom
        anchors.topMargin: 4
        anchors.bottom: parent.bottom
        x: 8
        width: parent.width - 16
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: TimelineModel
        visible: TimelineModel.count > 0
        currentIndex: -1
        keyNavigationEnabled: true
        highlightFollowsCurrentItem: false
        Keys.onUpPressed: (event) => {
            if (currentIndex <= 0) {
                searchField.forceActiveFocus()
                event.accepted = true
            } else {
                event.accepted = false
            }
        }

        onContentYChanged: {
            if (TimelineModel.hasMore && contentHeight > height && contentY + height >= contentHeight - 260)
                TimelineModel.loadMore()
        }

        delegate: Column {
            id: rowRoot
            required property int index
            required property string entryId
            required property string kind
            required property string direction
            required property bool incoming
            required property string deviceId
            required property string deviceName
            required property real timestamp
            required property real prevTimestamp
            required property string title
            required property string subtitle
            required property real sizeBytes
            required property int durationSecs
            required property int itemCount
            required property string thumbUrl
            required property bool canOpen
            required property bool canShowInFolder
            required property bool canCopy
            required property bool canSendAgain

            readonly property bool showDay: index === 0 || page.dayKey(timestamp) !== page.dayKey(prevTimestamp)
            readonly property bool isCurrent: page.current !== null && page.current.entryId === entryId
            // This row's roles, for the page's footer and Actions.
            readonly property var info: ({ entryId: entryId, kind: kind, title: title, canOpen: canOpen,
                                           canShowInFolder: canShowInFolder, canCopy: canCopy,
                                           canSendAgain: canSendAgain })

            function select() {
                listView.currentIndex = index
                page.current = info
            }
            // Keeps the page's copy current when this entry's roles change.
            onInfoChanged: if (isCurrent) page.current = info

            width: listView.width

            SectionLabel {
                visible: rowRoot.showDay
                width: parent.width
                height: visible ? (rowRoot.index === 0 ? 30 : 38) : 0
                text: rowRoot.showDay ? page.dayLabel(rowRoot.timestamp) : ""
            }

            ActionRow {
                id: entry
                width: parent.width
                iconPath: page.iconForRow(rowRoot.kind, rowRoot.title)
                tint: page.tintForKind(rowRoot.kind)
                title: rowRoot.title
                detail: rowRoot.subtitle
                selected: rowRoot.isCurrent
                activeFocusOnTab: false
                onClicked: {
                    rowRoot.select()
                    listView.forceActiveFocus()
                }
                onRightClicked: (x, y) => {
                    rowRoot.select()
                    menu.actions = page.rowActions(rowRoot.info)
                    const p = entry.mapToItem(page, x, y)
                    menu.openAt(p.x, p.y, false, false)
                }
                // Hover or selection shows the row's own buttons; otherwise
                // where it went and when.
                Row {
                    visible: entry.hovered || rowRoot.isCurrent
                    spacing: 2
                    RowButton {
                        visible: rowRoot.canOpen
                        iconPath: Icons.openExternal
                        label: qsTr("Open")
                        onClicked: TimelineModel.openEntry(rowRoot.entryId)
                    }
                    RowButton {
                        visible: rowRoot.canShowInFolder
                        iconPath: Icons.folder
                        label: qsTr("Show in folder")
                        onClicked: TimelineModel.revealEntry(rowRoot.entryId)
                    }
                    RowButton {
                        visible: rowRoot.canCopy
                        iconPath: Icons.copy
                        label: qsTr("Copy")
                        onClicked: TimelineModel.copyEntry(rowRoot.entryId)
                    }
                    RowButton {
                        visible: rowRoot.canSendAgain
                        iconPath: Icons.send
                        label: qsTr("Send again")
                        onClicked: TimelineModel.sendAgain(rowRoot.entryId, "")
                    }
                    RowButton {
                        iconPath: Icons.close
                        label: qsTr("Remove from timeline")
                        onClicked: page.removeEntry(rowRoot.entryId)
                    }
                }
                Txt {
                    visible: !(entry.hovered || rowRoot.isCurrent)
                    role: "bodySmall"
                    muted: true
                    text: page.directionText(rowRoot.incoming, rowRoot.deviceName)
                }
                Txt {
                    width: 64
                    horizontalAlignment: Text.AlignRight
                    role: "bodySmall"
                    muted: true
                    text: page.timeOfDay(rowRoot.timestamp)
                }
            }
        }

        // Keyboard moves the selection with the list's current row.
        onCurrentItemChanged: {
            if (currentItem && activeFocus)
                page.current = currentItem.info
        }

        footer: Item {
            width: listView.width
            height: TimelineModel.hasMore ? 48 : 16
            TextAction {
                anchors.centerIn: parent
                visible: TimelineModel.hasMore
                text: qsTr("Load more")
                onClicked: TimelineModel.loadMore()
            }
        }
    }

    PopupMenu {
        id: menu
        panelWidth: 260
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 6
        radius: Theme.radiusLg
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: opacity > 0
        opacity: AppController.highlightTarget === "timeline:main" ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }

    component RowButton: IconButton {
        width: 26
        height: 26
        iconSize: 15
    }
}
