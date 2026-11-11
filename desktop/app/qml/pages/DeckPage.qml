// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Dialogs
import app.g7link

// Deck: the tiles your phone shows (pages of one-tap actions run on this
// PC), edited here, with the PC's live sound state, both volumes and the
// phone's microphone in the right column.
Item {
    id: page
    property bool active: false
    property string deviceId: ""
    property string deviceName: ""
    onActiveChanged: DeckController.setPageActive(active)
    Component.onCompleted: if (active) DeckController.setPageActive(true)
    Component.onDestruction: DeckController.setPageActive(false)

    visible: active

    readonly property var pagesList: {
        try {
            const parsed = JSON.parse(DeckController.pagesJson)
            return Array.isArray(parsed) && parsed.length > 0 ? parsed : [{ id: "main", name: qsTr("Main"), tiles: [] }]
        } catch (e) {
            return [{ id: "main", name: qsTr("Main"), tiles: [] }]
        }
    }
    property int currentPageIndex: 0
    readonly property var currentPage: pagesList[Math.min(currentPageIndex, Math.max(0, pagesList.length - 1))] || { id: "main", name: qsTr("Main"), tiles: [] }
    readonly property var currentTiles: currentPage.tiles || []
    property int draggedTileIndex: -1
    // The tile the footer's Edit tile and Actions act on: the last one
    // focused or hovered.
    property int selectedTileIndex: 0
    readonly property var selectedTile: currentTiles.length > 0
        ? currentTiles[Math.min(selectedTileIndex, currentTiles.length - 1)] : null

    onPagesListChanged: {
        if (currentPageIndex >= pagesList.length)
            currentPageIndex = Math.max(0, pagesList.length - 1)
    }

    // ---- Footer and Actions (MainWindow reads these) ----
    readonly property string footerLabel: pagesList.length > 1
        ? qsTr("Deck · %1").arg(currentPage.name) : qsTr("Deck")
    readonly property var primaryAction: selectedTile
        ? { label: qsTr("Edit tile"), keys: "Enter", run: () => page.openEditTile(page.selectedTile) }
        : { label: qsTr("Add tile"), run: () => page.openAddTile() }
    readonly property var screenActions: {
        const list = [
            { label: qsTr("Add tile"), icon: Icons.plus, tint: Theme.accent, enabled: currentTiles.length < 24,
              run: () => page.openAddTile() }
        ]
        if (selectedTile) {
            const tile = selectedTile
            list.push({ label: qsTr("Edit tile"), detail: tile.label, icon: Icons.edit, tint: Theme.tintBlue, keys: "Enter",
                        run: () => page.openEditTile(tile) })
            list.push({ label: qsTr("Test tile"), detail: tile.label, icon: Icons.play, tint: Theme.tintGreen,
                        run: () => DeckController.testTile(tile.id) })
            list.push({ label: qsTr("Delete tile"), detail: tile.label, icon: Icons.trash, tint: Theme.tintRed,
                        run: () => DeckController.removeTile(page.currentPage.id, tile.id) })
        }
        for (let i = 0; i < pagesList.length; ++i) {
            const index = i
            list.push({ label: qsTr("Show page %1").arg(pagesList[i].name), icon: Icons.deck, tint: Theme.tintGreen,
                        checked: index === currentPageIndex, run: () => { page.currentPageIndex = index } })
        }
        list.push({ label: qsTr("New page"), icon: Icons.plus, tint: Theme.tintGreen, enabled: pagesList.length < 8,
                    run: () => page.openNewPage() })
        list.push({ label: qsTr("Rename page"), detail: currentPage.name, icon: Icons.edit, tint: Theme.surfaceContentVariant,
                    run: () => page.openRenamePage() })
        if (pagesList.length > 1)
            list.push({ label: qsTr("Delete page"), detail: currentPage.name, icon: Icons.trash, tint: Theme.tintRed,
                        run: () => DeckController.removePage(page.currentPage.id) })
        list.push({ label: qsTr("Reset Deck to defaults"), icon: Icons.refresh, tint: Theme.tintAmber,
                    run: () => resetConfirmSheet.open() })
        list.push({ label: PhoneMic.phase === "off" ? qsTr("Use the phone as a microphone") : qsTr("Stop the phone's microphone"),
                    icon: PhoneMic.phase === "off" ? Icons.mic : Icons.micOff, tint: Theme.tintCoral,
                    enabled: PhoneMic.phase !== "off" || PhoneMic.device !== "",
                    run: () => PhoneMic.phase === "off" ? PhoneMic.start(PhoneMic.device) : PhoneMic.stop() })
        return list
    }

    function openNewPage() {
        pageNameSheet.isNew = true
        pageNameInput.text = qsTr("Page %1").arg(page.pagesList.length + 1)
        pageNameSheet.open()
    }
    function openRenamePage() {
        pageNameSheet.isNew = false
        pageNameInput.text = page.currentPage.name
        pageNameSheet.open()
    }

    // A tile's own color, as one of the glyph tints.
    function tileTint(name) {
        switch (name) {
        case "violet": return Theme.tintLavender
        case "slate": return Theme.surfaceContentVariant
        case "amber": case "coral": case "red": case "teal": case "green": case "blue":
            return Theme.tint(name)
        default: return Theme.tintAmber
        }
    }

    function iconPathFor(name, kind) {
        if (kind === "media_play_pause" && DeckController.playing)
            return Icons.pause
        if (kind === "mic_mute" && DeckController.micState === 1)
            return Icons.micOff
        if ((kind === "volume_mute" || kind === "volume_up" || kind === "volume_down") && DeckController.muted)
            return Icons.soundOff
        switch (name) {
        case "play": return Icons.play
        case "pause": return Icons.pause
        case "skip_next":
        case "next": return Icons.skipNext
        case "skip_previous":
        case "prev": return Icons.skipPrevious
        case "volume_up": return Icons.volumeUp
        case "volume_down": return Icons.volumeDown
        case "volume_off":
        case "volume_mute": return Icons.soundOff
        case "mic": return Icons.mic
        case "mic_off": return Icons.micOff
        case "lock": return Icons.lock
        case "desktop": return Icons.desktop
        case "switch_window":
        case "window": return Icons.window
        case "screenshot":
        case "camera": return Icons.camera
        case "shortcut":
        case "keyboard": return Icons.keyboard
        case "globe": return Icons.globe
        case "text": return Icons.text
        case "app": return Icons.apps
        case "terminal": return Icons.terminal
        case "sparkle":
        case "bolt": return Icons.bolt
        case "star": return Icons.star
        case "music": return Icons.music
        case "folder": return Icons.folder
        case "video": return Icons.video
        case "bell": return Icons.bell
        default: return Icons.deck
        }
    }

    function isTileHighlighted(kind) {
        if (kind === "media_play_pause") return DeckController.playing
        if (kind === "volume_mute") return DeckController.muted
        if (kind === "mic_mute") return DeckController.micState === 1
        return false
    }

    readonly property var iconChoices: [
        "play", "pause", "skip_next", "skip_previous",
        "volume_up", "volume_down", "volume_off", "mic",
        "mic_off", "lock", "desktop", "switch_window",
        "screenshot", "shortcut", "globe", "text",
        "app", "terminal", "bolt", "star"
    ]

    readonly property var actionChoices: [
        { id: "media_play_pause", group: qsTr("Media"), label: qsTr("Play / pause") },
        { id: "media_previous",   group: qsTr("Media"), label: qsTr("Previous track") },
        { id: "media_next",       group: qsTr("Media"), label: qsTr("Next track") },
        { id: "volume_down",      group: qsTr("Audio"), label: qsTr("Volume down") },
        { id: "volume_up",        group: qsTr("Audio"), label: qsTr("Volume up") },
        { id: "volume_mute",      group: qsTr("Audio"), label: qsTr("Mute speaker") },
        { id: "mic_mute",         group: qsTr("Audio"), label: qsTr("Mute microphone") },
        { id: "show_desktop",     group: qsTr("System"), label: qsTr("Show desktop (Super+D)") },
        { id: "switch_window",    group: qsTr("System"), label: qsTr("Switch window (Alt+Tab)") },
        { id: "screenshot",       group: qsTr("System"), label: qsTr("Screenshot") },
        { id: "lock_pc",          group: qsTr("System"), label: qsTr("Lock PC") },
        { id: "shortcut",         group: qsTr("Custom"), label: qsTr("Keyboard shortcut") },
        { id: "open_url",         group: qsTr("Custom"), label: qsTr("Open website") },
        { id: "type_text",        group: qsTr("Custom"), label: qsTr("Type text snippet") },
        { id: "launch_app",       group: qsTr("Apps"),   label: qsTr("Launch application (.desktop / binary)") },
        { id: "run_command",      group: qsTr("Apps"),   label: qsTr("Run shell command") }
    ]

    function openAddTile() {
        editorSheet.tileId = ""
        editorSheet.actionKind = "shortcut"
        editorSheet.tileLabel = DeckController.defaultLabelFor("shortcut")
        editorSheet.labelCustomized = false
        editorSheet.tileIcon = DeckController.defaultIconFor("shortcut")
        editorSheet.tileColor = DeckController.defaultColorFor("shortcut")
        editorSheet.paramValue = "c"
        editorSheet.modCtrl = true
        editorSheet.modAlt = false
        editorSheet.modShift = false
        editorSheet.modWin = false
        editorSheet.errorText = ""
        editorSheet.open()
    }

    function openEditTile(tile) {
        if (!tile)
            return
        editorSheet.tileId = tile.id
        editorSheet.actionKind = tile.kind
        editorSheet.tileLabel = tile.label
        editorSheet.labelCustomized = true
        editorSheet.tileIcon = tile.icon
        editorSheet.tileColor = tile.color
        editorSheet.paramValue = tile.param || ""
        editorSheet.modCtrl = !!tile.ctrl
        editorSheet.modAlt = !!tile.alt
        editorSheet.modShift = !!tile.shift
        editorSheet.modWin = !!tile.win
        editorSheet.errorText = ""
        editorSheet.open()
    }

    // The phone's media volume, from its last reported controls.
    readonly property int phoneVolume: {
        const _rev = AppController.phoneTogglesRevision
        if (page.deviceId.length === 0)
            return -1
        try {
            const raw = AppController.phoneToggles(page.deviceId)
            if (raw.length === 0)
                return -1
            const t = JSON.parse(raw)
            return typeof t.volume === "number" ? t.volume : -1
        } catch (e) {
            return -1
        }
    }
    readonly property var micOutputs: {
        try {
            return JSON.parse(PhoneMic.outputs) || []
        } catch (e) {
            return []
        }
    }

    FileDialog {
        id: appFilePicker
        title: qsTr("Choose an application or shortcut")
        nameFilters: [qsTr("Applications and launchers (*.desktop *.AppImage *)"), qsTr("All files (*)")]
        onAccepted: {
            const localPath = DeckController.urlToLocalPath(selectedFile.toString())
            if (localPath.length > 0) {
                editorSheet.paramValue = localPath
                if (!editorSheet.labelCustomized || editorSheet.tileLabel.length === 0 || editorSheet.tileLabel === qsTr("Launch app")) {
                    const parts = localPath.split(/[\\/]/)
                    const file = parts[parts.length - 1] || ""
                    const stem = file.replace(/\.(exe|lnk|desktop|appimage)$/i, "")
                    if (stem.length > 0)
                        editorSheet.tileLabel = stem
                }
            }
        }
    }

    // ---- Tiles ----
    Flickable {
        id: tilesView
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.right: side.left
        contentHeight: tilesCol.height + 28
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: tilesCol
            x: 16
            y: 14
            width: tilesView.width - 32
            spacing: 10

            SectionLabel {
                width: parent.width
                text: qsTr("Tiles on your phone")
                actionText: qsTr("Add tile")
                actionIcon: Icons.plus
                onActionClicked: page.openAddTile()
            }

            // Pages: one tab each, then page actions.
            Item {
                width: parent.width
                height: 26
                Row {
                    spacing: 4
                    anchors.verticalCenter: parent.verticalCenter
                    Repeater {
                        model: page.pagesList
                        delegate: TextAction {
                            required property var modelData
                            required property int index
                            selected: index === page.currentPageIndex
                            text: "%1 · %2".arg(modelData.name).arg((modelData.tiles || []).length)
                            onClicked: page.currentPageIndex = index
                        }
                    }
                    TextAction {
                        visible: page.pagesList.length < 8
                        text: qsTr("New page")
                        iconPath: Icons.plus
                        onClicked: page.openNewPage()
                    }
                }
                Row {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4
                    TextAction {
                        text: qsTr("Rename")
                        onClicked: page.openRenamePage()
                    }
                    TextAction {
                        visible: page.pagesList.length > 1
                        text: qsTr("Delete page")
                        onClicked: DeckController.removePage(page.currentPage.id)
                    }
                    TextAction {
                        text: qsTr("Reset")
                        onClicked: resetConfirmSheet.open()
                    }
                }
            }

            Grid {
                id: tileGrid
                width: parent.width
                columns: width >= 560 ? 4 : 3
                spacing: 8

                readonly property real cellWidth: (width - spacing * (columns - 1)) / columns
                readonly property real cellHeight: Math.min(150, Math.max(92, Math.round(cellWidth / 1.15)))

                Repeater {
                    model: page.currentTiles
                    delegate: Item {
                        id: tileSlot
                        required property var modelData
                        required property int index

                        width: tileGrid.cellWidth
                        height: tileGrid.cellHeight
                        activeFocusOnTab: true
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.label
                        Accessible.description: modelData.subtitle || ""
                        Accessible.onPressAction: page.openEditTile(modelData)
                        onActiveFocusChanged: if (activeFocus) page.selectedTileIndex = index
                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                page.openEditTile(modelData)
                                event.accepted = true
                            }
                        }

                        readonly property bool isBeingDragged: page.draggedTileIndex === index
                        readonly property bool highlighted: page.isTileHighlighted(modelData.kind)
                        readonly property color tint: page.tileTint(modelData.color)

                        Rectangle {
                            anchors.fill: parent
                            radius: 10
                            color: tileHover.hovered || tileSlot.isBeingDragged ? Theme.selection : Theme.raised
                            border.width: tileSlot.isBeingDragged || Theme.focusVisible(tileSlot) ? 2 : (tileSlot.highlighted ? 1 : 0)
                            border.color: tileSlot.isBeingDragged || Theme.focusVisible(tileSlot) ? Theme.focusRing : tileSlot.tint
                            scale: tileSlot.isBeingDragged ? Theme.pressScale : 1.0
                            Behavior on scale { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
                        }
                        Icon {
                            x: 12; y: 12
                            width: 22; height: 22
                            path: page.iconPathFor(tileSlot.modelData.icon, tileSlot.modelData.kind)
                            color: tileSlot.tint
                        }
                        // Live state ("74%", "Muted", "Playing").
                        Txt {
                            visible: text.length > 0 && !testBtn.shown
                            anchors.right: parent.right
                            anchors.rightMargin: 12
                            y: 14
                            text: tileSlot.modelData.status || ""
                            role: "caption"
                            color: tileSlot.highlighted ? tileSlot.tint : Theme.surfaceContentVariant
                        }
                        Column {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            anchors.margins: 12
                            spacing: 0
                            Txt {
                                width: parent.width
                                text: tileSlot.modelData.label
                            }
                            Txt {
                                visible: text.length > 0
                                width: parent.width
                                text: tileSlot.modelData.subtitle || ""
                                role: "caption"
                                muted: true
                            }
                        }

                        HoverHandler {
                            id: tileHover
                            cursorShape: Qt.PointingHandCursor
                            onHoveredChanged: if (hovered) page.selectedTileIndex = tileSlot.index
                        }

                        // Drag to reorder, click to edit.
                        MouseArea {
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton
                            property point pressPos: Qt.point(0, 0)
                            property bool dragging: false

                            onPressed: (mouse) => {
                                pressPos = Qt.point(mouse.x, mouse.y)
                                dragging = false
                            }
                            onPositionChanged: (mouse) => {
                                if (!pressed) return
                                const dx = mouse.x - pressPos.x
                                const dy = mouse.y - pressPos.y
                                if (!dragging && (dx * dx + dy * dy > 64)) {
                                    dragging = true
                                    page.draggedTileIndex = tileSlot.index
                                }
                                if (dragging) {
                                    const inGrid = tileSlot.mapToItem(tileGrid, mouse.x, mouse.y)
                                    const col = Math.floor(inGrid.x / (tileGrid.cellWidth + tileGrid.spacing))
                                    const row = Math.floor(inGrid.y / (tileGrid.cellHeight + tileGrid.spacing))
                                    if (col >= 0 && col < tileGrid.columns && row >= 0) {
                                        const targetIdx = row * tileGrid.columns + col
                                        if (targetIdx >= 0 && targetIdx < page.currentTiles.length && targetIdx !== tileSlot.index) {
                                            DeckController.moveTile(page.currentPage.id, tileSlot.index, targetIdx)
                                            page.draggedTileIndex = targetIdx
                                        }
                                    }
                                }
                            }
                            onReleased: {
                                if (!dragging)
                                    page.openEditTile(tileSlot.modelData)
                                dragging = false
                                page.draggedTileIndex = -1
                            }
                            onCanceled: {
                                dragging = false
                                page.draggedTileIndex = -1
                            }
                        }

                        // Run the tile here, to try it.
                        IconButton {
                            id: testBtn
                            readonly property bool shown: tileHover.hovered || activeFocus
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: 6
                            opacity: shown ? 1 : 0
                            iconPath: Icons.play
                            iconSize: 14
                            iconColor: tileSlot.tint
                            label: qsTr("Test %1").arg(tileSlot.modelData.label)
                            onClicked: DeckController.testTile(tileSlot.modelData.id)
                        }
                    }
                }

                // Add a tile, at the end of the grid.
                Rectangle {
                    id: addTileBtn
                    visible: page.currentTiles.length < 24
                    width: tileGrid.cellWidth
                    height: tileGrid.cellHeight
                    radius: 10
                    color: addHover.hovered ? Theme.raised : "transparent"
                    border.width: Theme.focusVisible(addTileBtn) ? 2 : 1
                    border.color: Theme.focusVisible(addTileBtn) ? Theme.focusRing : Theme.line
                    activeFocusOnTab: true
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Add tile")
                    Accessible.onPressAction: page.openAddTile()
                    Keys.onPressed: (event) => {
                        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                            page.openAddTile()
                            event.accepted = true
                        }
                    }
                    Row {
                        anchors.centerIn: parent
                        spacing: 6
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 16; height: 16
                            path: Icons.plus
                            color: Theme.accent
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Add tile")
                            muted: true
                        }
                    }
                    HoverHandler { id: addHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: page.openAddTile() }
                }
            }

            Txt {
                width: parent.width
                text: qsTr("Drag to reorder. The phone sees only labels and icons.")
                role: "caption"
                muted: true
            }
        }
    }

    // ---- Right column: sound ----
    Flickable {
        id: side
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: Math.min(Theme.columnWidth + 20, Math.max(240, page.width * 0.32))
        contentHeight: sideCol.height + 28
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Rectangle {
            width: 1
            height: Math.max(side.height, side.contentHeight)
            color: Theme.line
        }

        Column {
            id: sideCol
            x: 12
            y: 10
            width: side.width - 24
            spacing: 2

            SectionLabel { width: parent.width; text: qsTr("This PC now") }
            ActionRow {
                width: parent.width
                interactive: false
                iconPath: DeckController.muted ? Icons.soundOff : Icons.speaker
                tint: Theme.tintTeal
                title: qsTr("Speaker")
                meta: DeckController.muted ? qsTr("Muted · %1%").arg(DeckController.volume) : qsTr("%1%").arg(DeckController.volume)
            }
            ActionRow {
                width: parent.width
                interactive: false
                iconPath: DeckController.micState === 1 ? Icons.micOff : Icons.mic
                tint: Theme.tintCoral
                title: qsTr("Microphone")
                meta: DeckController.micState === 1 ? qsTr("Muted")
                    : DeckController.micState === 0 ? qsTr("Live") : qsTr("None")
            }
            ActionRow {
                width: parent.width
                interactive: false
                iconPath: DeckController.playing ? Icons.play : Icons.pause
                tint: Theme.tintPink
                title: qsTr("Media")
                meta: DeckController.playing ? qsTr("Playing") : qsTr("Paused")
            }

            Item { width: 1; height: 8 }
            SectionLabel { width: parent.width; text: qsTr("Volume") }
            VolumeRow {
                width: parent.width
                name: qsTr("PC")
                value: DeckController.volume
                onMoved: (v) => MediaList.command("local", "", "volume", v)
            }
            VolumeRow {
                width: parent.width
                visible: page.phoneVolume >= 0
                name: qsTr("Phone")
                value: Math.max(0, page.phoneVolume)
                onMoved: (v) => AppController.setPhoneToggle(page.deviceId, "volume", String(v))
            }

            Item { width: 1; height: 8 }
            SectionLabel {
                width: parent.width
                text: qsTr("Phone as microphone")
                actionText: qsTr("Refresh")
                onActionClicked: PhoneMic.refresh()
            }
            ActionRow {
                width: parent.width
                iconPath: PhoneMic.phase === "off" ? Icons.mic : Icons.micOff
                tint: Theme.tintCoral
                enabled: PhoneMic.phase !== "off" || PhoneMic.device !== ""
                title: PhoneMic.phase === "live" ? qsTr("Stop") : (PhoneMic.phase === "asking" ? qsTr("Cancel") : qsTr("Start"))
                detail: PhoneMic.phase === "live" ? qsTr("Live from %1").arg(PhoneMic.deviceName)
                    : PhoneMic.phase === "asking" ? qsTr("Tap Start on %1").arg(PhoneMic.deviceName)
                    : PhoneMic.deviceName
                onClicked: PhoneMic.phase === "off" ? PhoneMic.start(PhoneMic.device) : PhoneMic.stop()
            }
            Txt {
                visible: page.micOutputs.length > 0
                x: Theme.rowPadding
                height: 24
                text: qsTr("Plays into")
                role: "caption"
                muted: true
            }
            Repeater {
                model: page.micOutputs
                delegate: RadioRow {
                    required property var modelData
                    width: sideCol.width
                    title: modelData.name
                    checked: modelData.id === PhoneMic.output
                    onClicked: PhoneMic.chooseOutput(modelData.id)
                }
            }
            VolumeRow {
                width: parent.width
                visible: PhoneMic.maxGain > PhoneMic.minGain
                name: qsTr("Gain")
                from: PhoneMic.minGain
                to: PhoneMic.maxGain
                value: PhoneMic.gain
                onMoved: (v) => PhoneMic.setLoudness(v)
            }
        }
    }

    // A radio choice: a lavender ring, filled when chosen.
    component RadioRow: ActionRow {
        id: radio
        property bool checked: false
        accessibleName: title
        Accessible.role: Accessible.RadioButton
        Accessible.checked: checked
        leading: Rectangle {
            width: 16; height: 16; radius: 8
            color: "transparent"
            border.width: radio.checked ? 5 : 1.5
            border.color: radio.checked ? Theme.accent : Theme.surfaceContentVariant
        }
    }

    // A labelled slider with its value, for the right column.
    component VolumeRow: Item {
        id: vol
        property string name
        property int value: 0
        property int from: 0
        property int to: 100
        signal moved(int value)
        height: 32
        Txt {
            id: volName
            x: Theme.rowPadding
            width: 56
            anchors.verticalCenter: parent.verticalCenter
            text: vol.name
            role: "bodySmall"
            muted: true
        }
        ValueSlider {
            anchors.left: volName.right
            anchors.right: volValue.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            from: vol.from
            to: vol.to
            value: vol.value
            label: vol.name
            valueText: qsTr("%1%").arg(vol.value)
            onMoved: (v) => vol.moved(v)
        }
        Txt {
            id: volValue
            anchors.right: parent.right
            anchors.rightMargin: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            width: 32
            horizontalAlignment: Text.AlignRight
            text: String(vol.value)
            role: "bodySmall"
        }
    }

    // ---- Add / Edit Tile Sheet ----
    Sheet {
        id: editorSheet
        cardWidth: 680

        property string tileId: ""
        property string actionKind: "shortcut"
        property string tileLabel: ""
        property bool labelCustomized: false
        property string tileIcon: "keyboard"
        property string tileColor: "amber"
        property string paramValue: ""
        property bool modCtrl: false
        property bool modAlt: false
        property bool modShift: false
        property bool modWin: false
        property string errorText: ""

        function selectKind(kind) {
            // Another kind of action: the last one's value and keys don't carry over.
            if (kind !== actionKind) {
                paramValue = ""
                modCtrl = false
                modAlt = false
                modShift = false
                modWin = false
            }
            actionKind = kind
            if (!labelCustomized || tileLabel.trim().length === 0) {
                tileLabel = DeckController.defaultLabelFor(kind)
            }
            tileIcon = DeckController.defaultIconFor(kind)
            tileColor = DeckController.defaultColorFor(kind)
            errorText = ""
            if (kind === "shortcut" && paramValue.length === 0) {
                paramValue = "c"
                modCtrl = true
            } else if (kind === "open_url" && paramValue.length === 0) {
                paramValue = "https://"
            }
        }

        Column {
            width: parent.width
            spacing: 10

            // Sheet Header + Live Tile Preview
            Row {
                width: parent.width
                spacing: 14

                Column {
                    width: parent.width - previewBox.width - 14
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4
                    Txt {
                        text: editorSheet.tileId.length === 0 ? qsTr("Add tile") : qsTr("Edit tile")
                        role: "headline"
                    }
                    Txt {
                        width: parent.width
                        wrapMode: Text.WordWrap
                        role: "bodySmall"
                        muted: true
                        text: qsTr("The phone sees only the label and icon.")
                    }
                }

                // Live miniature tile preview
                Rectangle {
                    id: previewBox
                    width: 148; height: 68
                    radius: 10
                    color: Theme.raised

                    Icon {
                        x: 10; y: 10
                        width: 20; height: 20
                        path: page.iconPathFor(editorSheet.tileIcon, editorSheet.actionKind)
                        color: page.tileTint(editorSheet.tileColor)
                    }
                    Txt {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        anchors.margins: 10
                        text: editorSheet.tileLabel.length > 0 ? editorSheet.tileLabel : DeckController.defaultLabelFor(editorSheet.actionKind)
                        elide: Text.ElideRight
                    }
                }
            }

            Divider { width: parent.width }

            // Action Picker
            Column {
                width: parent.width
                spacing: 6
                Txt { text: qsTr("Action"); role: "label"; muted: true }
                Flow {
                    id: actionFlow
                    width: parent.width
                    spacing: 6

                    Repeater {
                        model: page.actionChoices
                        delegate: Chip {
                            required property var modelData
                            interactive: true
                            selected: editorSheet.actionKind === modelData.id
                            text: modelData.label
                            onClicked: editorSheet.selectKind(modelData.id)
                        }
                    }
                }
            }

            // Parameter inputs for parameterized actions
            Column {
                width: parent.width
                spacing: 6
                visible: editorSheet.actionKind === "shortcut"
                      || editorSheet.actionKind === "open_url"
                      || editorSheet.actionKind === "type_text"
                      || editorSheet.actionKind === "launch_app"
                      || editorSheet.actionKind === "run_command"

                // Modifiers for Shortcut
                Row {
                    visible: editorSheet.actionKind === "shortcut"
                    spacing: 8
                    Chip {
                        interactive: true
                        selected: editorSheet.modCtrl
                        text: qsTr("Ctrl")
                        onClicked: editorSheet.modCtrl = !editorSheet.modCtrl
                    }
                    Chip {
                        interactive: true
                        selected: editorSheet.modAlt
                        text: qsTr("Alt")
                        onClicked: editorSheet.modAlt = !editorSheet.modAlt
                    }
                    Chip {
                        interactive: true
                        selected: editorSheet.modShift
                        text: qsTr("Shift")
                        onClicked: editorSheet.modShift = !editorSheet.modShift
                    }
                    Chip {
                        interactive: true
                        selected: editorSheet.modWin
                        text: qsTr("Super")
                        onClicked: editorSheet.modWin = !editorSheet.modWin
                    }
                }

                Txt {
                    role: "bodySmall"
                    muted: true
                    text: editorSheet.actionKind === "shortcut" ? qsTr("Key (e.g. c, v, z, f5, tab, enter, space, escape, up, down):")
                        : editorSheet.actionKind === "open_url" ? qsTr("Website address (https://…):")
                        : editorSheet.actionKind === "type_text" ? qsTr("Text snippet to type:")
                        : editorSheet.actionKind === "launch_app" ? (qsTr("Application or launcher path (/usr/bin/…, .desktop, or .AppImage):"))
                        : qsTr("Shell command to run on this PC:")
                }

                Row {
                    width: parent.width
                    spacing: 8

                    Rectangle {
                        width: editorSheet.actionKind === "launch_app" ? parent.width - browseBtn.width - 8 : parent.width
                        height: 36
                        radius: Theme.rowRadius
                        color: Theme.raised
                        border.width: paramInput.activeFocus ? 1 : 0
                        border.color: Theme.focusRing

                        TextInput {
                            id: paramInput
                            anchors.fill: parent
                            anchors.leftMargin: 12
                            anchors.rightMargin: 12
                            verticalAlignment: TextInput.AlignVCenter
                            clip: true
                            color: Theme.surfaceContent
                            selectionColor: Theme.selection
                            selectedTextColor: Theme.surfaceContent
                            font.family: editorSheet.actionKind === "run_command" || editorSheet.actionKind === "shortcut"
                                ? Theme.fontMono : Theme.fontUi
                            font.pixelSize: Theme.typography.body.size
                            text: editorSheet.paramValue
                            onTextEdited: editorSheet.paramValue = text
                            Accessible.name: qsTr("Action parameter")
                        }
                    }

                    Button {
                        id: browseBtn
                        visible: editorSheet.actionKind === "launch_app"
                        variant: "tonal"
                        size: "sm"
                        iconPath: Icons.folder
                        text: qsTr("Browse…")
                        onClicked: appFilePicker.open()
                    }
                }

                Txt {
                    visible: editorSheet.actionKind === "run_command"
                    width: parent.width
                    wrapMode: Text.WordWrap
                    role: "bodySmall"
                    color: Theme.warning
                    text: qsTr("Needs \"Allow running Deck commands\" for the phone in Settings.")
                }
            }

            // Tile Label Input
            Column {
                width: parent.width
                spacing: 6
                Txt { text: qsTr("Label"); role: "label"; muted: true }
                Rectangle {
                    width: parent.width
                    height: 36
                    radius: Theme.rowRadius
                    color: Theme.raised
                    border.width: labelInput.activeFocus ? 1 : 0
                    border.color: Theme.focusRing

                    TextInput {
                        id: labelInput
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        verticalAlignment: TextInput.AlignVCenter
                        clip: true
                        color: Theme.surfaceContent
                        selectionColor: Theme.selection
                        selectedTextColor: Theme.surfaceContent
                        font.family: Theme.fontUi
                        font.pixelSize: Theme.typography.body.size
                        text: editorSheet.tileLabel
                        onTextEdited: {
                            editorSheet.tileLabel = text
                            editorSheet.labelCustomized = true
                        }
                        Accessible.name: qsTr("Tile label")
                    }
                }
            }

            // Icon Picker
            Column {
                width: parent.width
                spacing: 6
                Txt { text: qsTr("Icon"); role: "label"; muted: true }
                Flow {
                    width: parent.width
                    spacing: 5
                    Repeater {
                        model: page.iconChoices
                        delegate: Rectangle {
                            id: iconCell
                            required property string modelData
                            readonly property bool chosen: editorSheet.tileIcon === modelData
                            width: 27; height: 27
                            radius: Theme.radiusSm
                            activeFocusOnTab: true
                            color: chosen ? Theme.selection : Theme.raised
                            border.width: chosen || Theme.focusVisible(iconCell) ? 1 : 0
                            border.color: Theme.focusRing
                            Accessible.role: Accessible.RadioButton
                            Accessible.name: modelData
                            Accessible.checked: chosen
                            Accessible.onPressAction: editorSheet.tileIcon = modelData
                            Keys.onPressed: (event) => {
                                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                    editorSheet.tileIcon = modelData
                                    event.accepted = true
                                }
                            }
                            Icon {
                                anchors.centerIn: parent
                                width: 15; height: 15
                                path: page.iconPathFor(modelData, "")
                                color: chosen ? page.tileTint(editorSheet.tileColor) : Theme.surfaceContent
                            }
                            HoverHandler { cursorShape: Qt.PointingHandCursor }
                            TapHandler { onTapped: editorSheet.tileIcon = modelData }
                        }
                    }
                }
            }

            Txt {
                visible: editorSheet.errorText.length > 0
                width: parent.width
                wrapMode: Text.WordWrap
                role: "bodySmall"
                color: Theme.error
                text: editorSheet.errorText
            }

            // Sheet Actions
            Item {
                width: parent.width
                height: 38

                Row {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    visible: editorSheet.tileId.length > 0

                    Button {
                        variant: "outline"
                        size: "sm"
                        iconPath: Icons.trash
                        text: qsTr("Delete")
                        onClicked: {
                            DeckController.removeTile(page.currentPage.id, editorSheet.tileId)
                            editorSheet.close()
                        }
                    }
                    Button {
                        variant: "tonal"
                        size: "sm"
                        iconPath: Icons.bolt
                        text: qsTr("Test")
                        onClicked: DeckController.testTile(editorSheet.tileId)
                    }
                }

                Row {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8

                    Button {
                        variant: "text"
                        text: qsTr("Cancel")
                        onClicked: editorSheet.close()
                    }
                    Button {
                        variant: "fill"
                        text: qsTr("Save")
                        onClicked: {
                            const err = DeckController.saveTile(
                                page.currentPage.id,
                                editorSheet.tileId,
                                editorSheet.tileLabel,
                                editorSheet.tileIcon,
                                editorSheet.tileColor,
                                editorSheet.actionKind,
                                editorSheet.paramValue,
                                editorSheet.modCtrl,
                                editorSheet.modAlt,
                                editorSheet.modShift,
                                editorSheet.modWin
                            )
                            if (err.length > 0) {
                                editorSheet.errorText = err
                            } else {
                                editorSheet.close()
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- Add / Rename Page Sheet ----
    Sheet {
        id: pageNameSheet
        property bool isNew: true
        cardWidth: 380

        function submitPageName() {
            const trimmed = pageNameInput.text.trim()
            if (trimmed.length === 0) return
            if (pageNameSheet.isNew) {
                DeckController.addPage(trimmed)
                page.currentPageIndex = page.pagesList.length - 1
            } else {
                DeckController.renamePage(page.currentPage.id, trimmed)
            }
            pageNameSheet.close()
        }

        Column {
            width: parent.width
            spacing: 14

            Txt {
                text: pageNameSheet.isNew ? qsTr("New page") : qsTr("Rename page")
                role: "headline"
            }

            Rectangle {
                width: parent.width
                height: 38
                radius: Theme.rowRadius
                color: Theme.raised
                border.width: pageNameInput.activeFocus ? 1 : 0
                border.color: Theme.focusRing

                TextInput {
                    id: pageNameInput
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 12
                    verticalAlignment: TextInput.AlignVCenter
                    clip: true
                    color: Theme.surfaceContent
                    selectionColor: Theme.selection
                    selectedTextColor: Theme.surfaceContent
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.body.size
                    Accessible.name: qsTr("Page name")
                    Keys.onReturnPressed: pageNameSheet.submitPageName()
                    Keys.onEnterPressed: pageNameSheet.submitPageName()
                }
            }

            Row {
                anchors.right: parent.right
                spacing: 8
                Button { variant: "text"; text: qsTr("Cancel"); onClicked: pageNameSheet.close() }
                Button {
                    variant: "fill"
                    text: pageNameSheet.isNew ? qsTr("Create") : qsTr("Save")
                    onClicked: pageNameSheet.submitPageName()
                }
            }
        }
    }

    // ---- Reset Confirmation Sheet ----
    Sheet {
        id: resetConfirmSheet
        cardWidth: 400
        Column {
            width: parent.width
            spacing: 14
            Txt { text: qsTr("Reset Deck to defaults?"); role: "headline" }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                role: "body"
                muted: true
                text: qsTr("Your pages and tiles give way to the default 11 tiles.")
            }
            Row {
                anchors.right: parent.right
                spacing: 8
                Button { variant: "text"; text: qsTr("Cancel"); onClicked: resetConfirmSheet.close() }
                Button {
                    variant: "fill"
                    text: qsTr("Reset")
                    onClicked: {
                        DeckController.resetDefault()
                        page.currentPageIndex = 0
                        resetConfirmSheet.close()
                    }
                }
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 10
        radius: Theme.radiusLg
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: opacity > 0
        opacity: AppController.highlightTarget === "deck:main" ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }
}
