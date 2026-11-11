// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The command palette: a 16 px search, results grouped under muted labels,
// and a footer with the G7 mark, the phone and what Enter and Esc do.
// Loaded by MainWindow only while open, so it costs nothing while closed.
Item {
    id: commandPalette
    anchors.fill: parent
    z: 150

    property int selectedIndex: 0
    // Results grouped by category, in the order each first appears (the
    // best match's group first), each group's first row carrying its label.
    readonly property var items: {
        const raw = AppController.commandPaletteResults
        if (!raw || raw.length === 0)
            return []
        let list = []
        try {
            list = JSON.parse(raw)
        } catch (e) {
            return []
        }
        const order = []
        const groups = {}
        for (const item of list) {
            const key = item.category || ""
            if (!(key in groups)) {
                groups[key] = []
                order.push(key)
            }
            groups[key].push(item)
        }
        const out = []
        for (const key of order)
            groups[key].forEach((item, i) => out.push(Object.assign({}, item, { firstOfGroup: i === 0 && key.length > 0 })))
        return out
    }

    // Each result's glyph in its own tint: by its icon, else by its group.
    function tintFor(item) {
        const byIcon = {
            camera: Theme.tintCoral, photo: Theme.tintCoral, clipboard: Theme.tintLavender,
            clipboardList: Theme.tintLavender, apps: Theme.tintPink, mirror: Theme.tintPink, music: Theme.tintPink,
            call: Theme.tintTeal, callIncoming: Theme.tintTeal, callOutgoing: Theme.tintTeal, person: Theme.tintTeal,
            messages: Theme.tintBlue, bell: Theme.tintBlue, send: Theme.tintAmber, folder: Theme.tintAmber,
            download: Theme.tintAmber, power: Theme.tintRed, callMissed: Theme.tintRed, lock: Theme.tintTeal,
            deck: Theme.tintGreen, battery: Theme.tintGreen, settings: Theme.surfaceContentVariant
        }
        if (item.icon && byIcon[item.icon] !== undefined)
            return byIcon[item.icon]
        const byGroup = {
            Calls: Theme.tintTeal, Messages: Theme.tintBlue, Clipboard: Theme.tintLavender,
            "Mirror Apps": Theme.tintPink, Navigation: Theme.surfaceContentVariant
        }
        return byGroup[item.category] !== undefined ? byGroup[item.category] : Theme.accent
    }

    onItemsChanged: {
        if (selectedIndex >= items.length)
            selectedIndex = Math.max(0, items.length - 1)
    }

    function runSelected() {
        if (items.length === 0)
            return
        const idx = Math.max(0, Math.min(selectedIndex, items.length - 1))
        const item = items[idx]
        if (item && item.id)
            AppController.runCommandPalette(item.id)
    }

    ParallelAnimation {
        id: openAnim
        NumberAnimation {
            target: commandPalette
            property: "opacity"
            from: 0.45
            to: 1.0
            duration: Theme.fadeFast
            easing.type: Easing.OutCubic
        }
        NumberAnimation {
            target: card
            property: "scale"
            from: 0.97
            to: 1.0
            duration: Theme.fadeFast
            easing.type: Easing.OutCubic
        }
    }

    Component.onCompleted: {
        if (!Theme.reduceMotion)
            openAnim.restart()
        searchInput.forceActiveFocus()
        Qt.callLater(() => AppController.noteCommandPaletteShown())
    }

    Shortcut {
        sequence: "Esc"
        enabled: commandPalette.visible
        onActivated: AppController.closeCommandPalette()
    }

    Rectangle {
        id: scrim
        anchors.fill: parent
        color: Theme.scrim
        TapHandler {
            onTapped: (eventPoint) => {
                const p = card.mapFromItem(scrim, eventPoint.position)
                const inside = p.x >= 0 && p.y >= 0 && p.x <= card.width && p.y <= card.height
                if (!inside)
                    AppController.closeCommandPalette()
            }
        }
    }

    Rectangle {
        id: card
        anchors.horizontalCenter: parent.horizontalCenter
        y: Math.max(24, Math.round((parent.height - height) * 0.3))
        width: Math.min(760, parent.width - 48)
        height: Math.min(520, parent.height - 48)
        radius: Theme.radiusXl
        color: Theme.surface
        border.width: 1
        border.color: Theme.line
        clip: true

        // ---- Search ----
        Item {
            id: searchBar
            width: parent.width
            height: 56

            Txt {
                anchors.left: searchInput.left
                anchors.verticalCenter: parent.verticalCenter
                visible: searchInput.text.length === 0
                text: qsTr("Search contacts, messages, apps and actions")
                size: 16
                muted: true
            }

            TextInput {
                id: searchInput
                anchors.left: parent.left
                anchors.leftMargin: 18
                anchors.right: escBadge.left
                anchors.rightMargin: 10
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                verticalAlignment: TextInput.AlignVCenter
                font.family: Theme.fontUi
                font.pixelSize: 16
                color: Theme.surfaceContent
                selectionColor: Theme.selection
                selectedTextColor: Theme.surfaceContent
                focus: true
                clip: true
                Accessible.name: qsTr("Command palette search")

                onTextChanged: {
                    commandPalette.selectedIndex = 0
                    AppController.searchCommandPalette(searchInput.text)
                }

                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Down) {
                        if (commandPalette.items.length > 0) {
                            commandPalette.selectedIndex = (commandPalette.selectedIndex + 1) % commandPalette.items.length
                            resultsList.positionViewAtIndex(commandPalette.selectedIndex, ListView.Contain)
                        }
                        event.accepted = true
                    } else if (event.key === Qt.Key_Up) {
                        if (commandPalette.items.length > 0) {
                            commandPalette.selectedIndex = (commandPalette.selectedIndex - 1 + commandPalette.items.length) % commandPalette.items.length
                            resultsList.positionViewAtIndex(commandPalette.selectedIndex, ListView.Contain)
                        }
                        event.accepted = true
                    } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                        commandPalette.runSelected()
                        event.accepted = true
                    } else if (event.key === Qt.Key_Escape) {
                        AppController.closeCommandPalette()
                        event.accepted = true
                    }
                }
            }

            // Clears the search, or closes the palette when it's empty.
            Keycap {
                id: escBadge
                anchors.right: parent.right
                anchors.rightMargin: 18
                anchors.verticalCenter: parent.verticalCenter
                keys: searchInput.text.length > 0 ? qsTr("Clear") : "Esc"
                activeFocusOnTab: true

                function activate() {
                    if (searchInput.text.length > 0) {
                        searchInput.text = ""
                        searchInput.forceActiveFocus()
                    } else {
                        AppController.closeCommandPalette()
                    }
                }

                Accessible.role: Accessible.Button
                Accessible.name: searchInput.text.length > 0 ? qsTr("Clear search") : qsTr("Close command palette")
                Accessible.onPressAction: escBadge.activate()
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                        escBadge.activate()
                        event.accepted = true
                    }
                }
                border.width: Theme.focusVisible(escBadge) ? 2 : 0
                border.color: Theme.focusRing

                HoverHandler { cursorShape: Qt.PointingHandCursor }
                TapHandler { onTapped: escBadge.activate() }
            }

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: 1
                color: Theme.line
            }
        }

        // ---- Results, grouped ----
        ListView {
            id: resultsList
            anchors.top: searchBar.bottom
            anchors.bottom: footer.top
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: 8
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            visible: commandPalette.items.length > 0
            model: commandPalette.items

            delegate: Item {
                id: row
                required property var modelData
                required property int index
                readonly property bool selected: index === commandPalette.selectedIndex
                readonly property bool isEnabled: modelData.enabled !== false
                readonly property bool firstOfGroup: modelData.firstOfGroup === true

                width: resultsList.width
                height: (firstOfGroup ? groupLabel.height : 0) + Theme.rowHeight

                Txt {
                    id: groupLabel
                    visible: row.firstOfGroup
                    x: Theme.rowPadding
                    height: 28
                    verticalAlignment: Text.AlignBottom
                    bottomPadding: 4
                    text: row.modelData.category || ""
                    role: "label"
                    muted: true
                }

                Rectangle {
                    id: rowBody
                    anchors.bottom: parent.bottom
                    width: parent.width
                    height: Theme.rowHeight
                    radius: Theme.rowRadius
                    color: row.selected ? Theme.selection : "transparent"

                    Accessible.role: Accessible.ListItem
                    Accessible.name: (row.modelData.title || "") + (row.modelData.subtitle ? ", " + row.modelData.subtitle : "")
                    Accessible.onPressAction: {
                        commandPalette.selectedIndex = row.index
                        AppController.runCommandPalette(row.modelData.id)
                    }

                    HoverHandler {
                        cursorShape: Qt.PointingHandCursor
                        onHoveredChanged: {
                            if (hovered)
                                commandPalette.selectedIndex = row.index
                        }
                    }
                    TapHandler {
                        onTapped: {
                            commandPalette.selectedIndex = row.index
                            AppController.runCommandPalette(row.modelData.id)
                        }
                    }

                    Icon {
                        id: glyph
                        x: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        width: 18; height: 18
                        path: (row.modelData.icon && Icons[row.modelData.icon]) ? Icons[row.modelData.icon] : Icons.terminal
                        color: commandPalette.tintFor(row.modelData)
                        opacity: row.isEnabled ? 1.0 : 0.6
                    }
                    Txt {
                        id: rowTitle
                        anchors.left: glyph.right
                        anchors.leftMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(implicitWidth, kind.x - x - 12 - (rowSub.text.length > 0 ? Math.min(rowSub.implicitWidth, 160) + 10 : 0))
                        text: row.modelData.title || ""
                        opacity: row.isEnabled ? 1.0 : 0.6
                    }
                    Txt {
                        id: rowSub
                        anchors.left: rowTitle.right
                        anchors.leftMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.max(0, kind.x - x - 12)
                        text: row.modelData.subtitle || ""
                        muted: true
                    }
                    Txt {
                        id: kind
                        anchors.right: parent.right
                        anchors.rightMargin: Theme.rowPadding
                        anchors.verticalCenter: parent.verticalCenter
                        text: row.modelData.badge || ""
                        role: "bodySmall"
                        muted: true
                    }
                }
            }
        }

        Txt {
            anchors.centerIn: resultsList
            visible: commandPalette.items.length === 0
            text: qsTr("No matches")
            muted: true
        }

        // ---- Footer: the mark, the phone, and what Enter and Esc do ----
        Item {
            id: footer
            anchors.bottom: parent.bottom
            width: parent.width
            height: 40

            Rectangle {
                width: parent.width
                height: 1
                color: Theme.line
            }
            Row {
                x: 14
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 20; height: 20
                    path: Icons.mark
                    color: Theme.accent
                    stroke: 2
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: text.length > 0
                    text: DeviceList.count > 0
                        ? DeviceList.deviceNameAt(Math.min(Math.max(0, AppController.currentDevice), DeviceList.count - 1))
                        : ""
                    role: "bodySmall"
                    muted: true
                }
                // How quickly it opened, beside its key.
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: {
                        const hk = Preferences.commandPaletteHotkey.length > 0
                            ? Preferences.commandPaletteHotkey
                            : "Ctrl+K"
                        if (AppController.commandPaletteOpenMs <= 0)
                            return hk
                        const msText = AppController.commandPaletteOpenMs < 10
                            ? Qt.locale().toString(AppController.commandPaletteOpenMs, "f", 1)
                            : String(Math.round(AppController.commandPaletteOpenMs))
                        return qsTr("%1 · %2 ms").arg(hk).arg(msText)
                    }
                    role: "bodySmall"
                    muted: true
                }
            }
            Row {
                anchors.right: parent.right
                anchors.rightMargin: 14
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: commandPalette.items.length > 0
                    text: qsTr("Open")
                    role: "bodySmall"
                }
                Keycap {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: commandPalette.items.length > 0
                    keys: "Enter"
                }
                Item { width: 8; height: 1 }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Close")
                    role: "bodySmall"
                    muted: true
                }
                Keycap {
                    anchors.verticalCenter: parent.verticalCenter
                    keys: "Esc"
                }
            }
        }
    }
}
