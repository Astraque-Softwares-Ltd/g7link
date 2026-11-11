// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import QtQuick.Shapes
import app.g7link

// Slide-out edge panel ("The Shelf"): the latest photo and screenshot, the
// clipboard, recent received files, and a drop zone. Dragging
// anything into the panel sends it to the connected phone; dragging any card
// out starts a native OS drag so it can be dropped into Explorer, browsers,
// or editors. Styled with WS_EX_NOACTIVATE so it never steals focus from the
// active application, and only instantiated while AppController.shelfOpen is
// true so nothing runs while hidden.
Window {
    id: shelfWin

    title: qsTr("G7 Link Shelf")
    flags: Qt.Tool | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.WindowDoesNotAcceptFocus
    color: "transparent"
    visible: true

    readonly property int availW: Screen.desktopAvailableWidth > 0 ? Screen.desktopAvailableWidth : 1920
    readonly property int availH: Screen.desktopAvailableHeight > 0 ? Screen.desktopAvailableHeight : 1080

    // The work area (the screen minus the taskbar, wherever the taskbar is). Qt only knows
    // its size, so the app reports the position; refreshed when the work area changes.
    property var workArea: AppController.workArea()
    readonly property int workAreaKey: Screen.desktopAvailableWidth * 100000 + Screen.desktopAvailableHeight
    onWorkAreaKeyChanged: workArea = AppController.workArea()
    readonly property real dpr: Screen.devicePixelRatio > 0 ? Screen.devicePixelRatio : 1
    readonly property real areaX: workArea && workArea.width ? workArea.x / dpr : Screen.virtualX
    readonly property real areaY: workArea && workArea.height ? workArea.y / dpr : Screen.virtualY
    readonly property real areaW: workArea && workArea.width ? workArea.width / dpr : availW
    readonly property real areaH: workArea && workArea.height ? workArea.height / dpr : availH

    width: 380
    height: Math.min(640, Math.max(420, availH - 48))
    x: areaX + areaW - width - 12
    y: areaY + Math.round((areaH - height) / 2)

    property bool justDropped: false
    property bool undoPending: false
    property bool undoCancelled: false
    property var pendingUrls: []
    property string pendingText: ""
    property string pendingLabel: ""
    /// An image clip waiting out the undo; it's sent from clipboard history.
    property string pendingClipId: ""
    property real undoProgress: 0.0
    property double undoStartedAt: 0

    readonly property string targetPhoneName: shelfData.hasDevice && shelfData.deviceName.length > 0
        ? shelfData.deviceName
        : qsTr("phone")

    function queueSendToPhone(urls, text, label, clipId) {
        const cleanUrls = urls ? urls : []
        const cleanText = text ? String(text) : ""
        const cleanClip = clipId ? String(clipId) : ""
        if (cleanUrls.length === 0 && cleanText.trim().length === 0 && cleanClip.length === 0)
            return
        if (!shelfData.hasDevice) {
            // Nothing to send to: the app says no phone is paired yet.
            AppController.dropToShelf(cleanUrls, cleanText)
            return
        }
        if (undoPending) {
            commitPendingSend()
        }
        pendingUrls = cleanUrls
        pendingText = cleanText
        pendingLabel = label ? String(label) : ""
        pendingClipId = cleanClip
        undoCancelled = false
        justDropped = false
        undoPending = true
        undoProgress = 1.0
        undoStartedAt = Date.now()
        if (Theme.reduceMotion) {
            undoStepTimer.restart()
        } else {
            undoProgressAnim.restart()
        }
        undoSendTimer.restart()
    }

    function commitPendingSend() {
        if (!undoPending)
            return
        undoSendTimer.stop()
        undoProgressAnim.stop()
        undoStepTimer.stop()
        const urls = pendingUrls
        const text = pendingText
        const clipId = pendingClipId
        undoPending = false
        pendingUrls = []
        pendingText = ""
        pendingLabel = ""
        pendingClipId = ""
        undoProgress = 0.0
        if (clipId.length > 0) {
            AppController.sendClipboardHistory(shelfData.deviceId, clipId)
            justDropped = true
            dropConfirmTimer.restart()
        } else if (urls.length > 0 || text.trim().length > 0) {
            AppController.dropToShelf(urls, text)
            justDropped = true
            dropConfirmTimer.restart()
        }
    }

    function cancelPendingSend() {
        if (!undoPending)
            return
        undoSendTimer.stop()
        undoProgressAnim.stop()
        undoStepTimer.stop()
        undoPending = false
        pendingUrls = []
        pendingText = ""
        pendingLabel = ""
        pendingClipId = ""
        undoProgress = 0.0
        justDropped = false
        undoCancelled = true
        dropConfirmTimer.restart()
    }

    readonly property var shelfData: {
        if (!AppController.shelfItems || AppController.shelfItems === "") {
            return ({
                hasDevice: false,
                deviceName: "",
                latestPhoto: null,
                latestScreenshot: null,
                currentClip: null,
                recentFiles: []
            })
        }
        try {
            return JSON.parse(AppController.shelfItems)
        } catch (e) {
            return ({
                hasDevice: false,
                deviceName: "",
                latestPhoto: null,
                latestScreenshot: null,
                currentClip: null,
                recentFiles: []
            })
        }
    }

    // "2 min ago", from Unix seconds or milliseconds.
    function relativeTime(date) {
        const t = Number(date) || 0
        if (t <= 0)
            return ""
        const ms = t > 1e12 ? t : t * 1000
        const mins = Math.max(0, Math.round((Date.now() - ms) / 60000))
        if (mins < 1)
            return qsTr("Just now")
        if (mins < 60)
            return qsTr("%1 min ago").arg(mins)
        const hours = Math.round(mins / 60)
        if (hours < 24)
            return qsTr("%1 h ago").arg(hours)
        return qsTr("%1 d ago").arg(Math.round(hours / 24))
    }

    function formatBytes(bytes) {
        const b = Number(bytes) || 0
        const locale = Qt.locale()
        if (b < 1024)
            return qsTr("%1 B").arg(b)
        if (b < 1024 * 1024)
            return qsTr("%1 KB").arg(locale.toString(b / 1024, "f", 1))
        if (b < 1024 * 1024 * 1024)
            return qsTr("%1 MB").arg(locale.toString(b / (1024 * 1024), "f", 1))
        return qsTr("%1 GB").arg(locale.toString(b / (1024 * 1024 * 1024), "f", 2))
    }

    Component.onCompleted: {
        Qt.callLater(() => AppController.styleShelfWindow())
        if (!Theme.reduceMotion) {
            slideAnim.restart()
        }
    }

    onVisibleChanged: {
        if (visible) {
            Qt.callLater(() => AppController.styleShelfWindow())
        } else if (undoPending) {
            commitPendingSend()
        }
    }

    // No keyboard shortcuts: the window never takes focus
    // (WindowDoesNotAcceptFocus), so window shortcuts could never fire, and
    // app-wide ones would take Ctrl+Z from the app you're working in.

    Timer {
        id: undoSendTimer
        interval: 3000
        repeat: false
        onTriggered: shelfWin.commitPendingSend()
    }

    NumberAnimation {
        id: undoProgressAnim
        target: shelfWin
        property: "undoProgress"
        from: 1.0
        to: 0.0
        duration: 3000
        easing.type: Easing.Linear
    }

    Timer {
        id: undoStepTimer
        interval: 250
        repeat: true
        onTriggered: shelfWin.undoProgress = Math.max(0, 1 - (Date.now() - shelfWin.undoStartedAt) / undoSendTimer.interval)
    }

    Timer {
        id: dropConfirmTimer
        interval: 1800
        onTriggered: {
            shelfWin.justDropped = false
            shelfWin.undoCancelled = false
        }
    }

    Item {
        id: rootItem
        anchors.fill: parent

        transform: Translate {
            id: slideTransform
            x: 0
        }

        NumberAnimation {
            id: slideAnim
            target: slideTransform
            property: "x"
            from: 24
            to: 0
            duration: Theme.reduceMotion ? 0 : 180
            easing.type: Easing.OutCubic
        }

        Rectangle {
            id: panel
            anchors.fill: parent
            radius: Theme.radiusXl
            color: Theme.surface
            border.width: 1
            border.color: shelfDropArea.containsDrag ? Theme.accent : Theme.line

            Behavior on border.color {
                enabled: !Theme.reduceMotion
                ColorAnimation { duration: Theme.fadeFast }
            }

            // Whole-window drop target so dropping anywhere on the Shelf queues a send with 3s Undo.
            DropArea {
                id: shelfDropArea
                anchors.fill: parent
                onDropped: (drop) => {
                    const urls = []
                    if (drop.urls) {
                        for (let i = 0; i < drop.urls.length; i++) {
                            urls.push(drop.urls[i].toString())
                        }
                    }
                    const text = drop.text ? String(drop.text) : ""
                    if (urls.length > 0 || text.trim().length > 0) {
                        shelfWin.queueSendToPhone(urls, text, "")
                    }
                    drop.acceptProposedAction()
                }
            }

            // ---- Header: Shelf, the phone, its key ----
            Item {
                id: header
                width: parent.width
                height: 44

                // Wayland ignores where we ask to put the window, so let it be dragged.
                DragHandler {
                    target: null
                    enabled: Qt.platform.pluginName === "wayland"
                    onActiveChanged: if (active) shelfWin.startSystemMove()
                }

                Row {
                    x: 14
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Shelf")
                        weight: 500
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: shelfWin.shelfData.hasDevice ? shelfWin.shelfData.deviceName : qsTr("No phone connected")
                        role: "bodySmall"
                        muted: true
                    }
                }

                Row {
                    anchors.right: parent.right
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2

                    Keycap {
                        anchors.verticalCenter: parent.verticalCenter
                        keys: (Preferences.shelfHotkey || "").split("+").join(" ")
                    }
                    Item { width: 4; height: 1 }
                    IconButton {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 26; height: 26
                        iconSize: 15
                        iconPath: Icons.refresh
                        label: qsTr("Refresh")
                        onClicked: AppController.refreshShelf()
                    }
                    IconButton {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 26; height: 26
                        iconSize: 15
                        iconPath: Icons.close
                        label: qsTr("Close")
                        onClicked: AppController.closeShelf()
                    }
                }

                Rectangle {
                    anchors.bottom: parent.bottom
                    width: parent.width
                    height: 1
                    color: Theme.line
                }
            }

            // ---- Photos, the clipboard and received files ----
            Flickable {
                id: contentFlick
                anchors.top: header.bottom
                anchors.bottom: dropZone.top
                anchors.bottomMargin: 10
                width: parent.width
                contentWidth: width
                contentHeight: cardsColumn.implicitHeight + 12
                clip: true
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: cardsColumn
                    x: 14
                    y: 12
                    width: parent.width - 28
                    spacing: 8

                    // ---- Latest photo and latest screenshot ----
                    Row {
                        width: parent.width
                        spacing: 8

                        Repeater {
                            model: [
                                {
                                    slotKind: "photo",
                                    title: qsTr("Latest photo"),
                                    emptyText: qsTr("None yet"),
                                    icon: Icons.photo,
                                    tint: Theme.tintCoral,
                                    item: shelfWin.shelfData.latestPhoto
                                },
                                {
                                    slotKind: "screenshot",
                                    title: qsTr("Latest screenshot"),
                                    emptyText: qsTr("None yet"),
                                    icon: Icons.camera,
                                    tint: Theme.tintBlue,
                                    item: shelfWin.shelfData.latestScreenshot
                                }
                            ]

                            delegate: Rectangle {
                                id: mediaCard
                                required property var modelData
                                readonly property var itemData: modelData.item
                                readonly property bool hasItem: itemData !== null && itemData !== undefined
                                readonly property string thumbSource: hasItem ? (itemData.thumbUrl || itemData.thumbnailDataUrl || "") : ""
                                readonly property string targetRef: hasItem ? (itemData.localPath || itemData.id || "") : ""
                                // Only a file on this PC can be sent: a photo still on the phone
                                // has just a thumbnail here, which isn't the picture.
                                readonly property string sendUrl: hasItem && itemData.localPath ? itemData.localPath : ""

                                property real flickOffsetX: 0
                                property real flickOffsetY: 0

                                width: Math.floor((cardsColumn.width - 8) / 2)
                                height: width
                                radius: Theme.rowRadius
                                clip: true
                                color: Theme.wash(modelData.tint)
                                border.width: mediaDragArea.flicking ? 2 : 0
                                border.color: Theme.accent
                                z: (flickOffsetY !== 0 || mediaDragArea.flicking) ? 10 : 0

                                transform: Translate {
                                    x: mediaCard.flickOffsetX
                                    y: mediaCard.flickOffsetY
                                }

                                Behavior on flickOffsetX {
                                    enabled: !Theme.reduceMotion && !mediaDragArea.pressed
                                    SpringAnimation { spring: 4.5; damping: 0.35; epsilon: 0.25 }
                                }

                                Behavior on flickOffsetY {
                                    enabled: !Theme.reduceMotion && !mediaDragArea.pressed && !mediaFlickAnim.running
                                    SpringAnimation { spring: 4.5; damping: 0.35; epsilon: 0.25 }
                                }

                                SequentialAnimation {
                                    id: mediaFlickAnim
                                    NumberAnimation {
                                        target: mediaCard
                                        property: "flickOffsetY"
                                        to: -54
                                        duration: Theme.reduceMotion ? 0 : 140
                                        easing.type: Easing.OutBack
                                    }
                                    SpringAnimation {
                                        target: mediaCard
                                        property: "flickOffsetY"
                                        to: 0
                                        spring: 4.5
                                        damping: 0.35
                                        epsilon: 0.25
                                    }
                                }

                                Icon {
                                    anchors.centerIn: parent
                                    visible: !thumb.visible
                                    width: 22; height: 22
                                    path: mediaCard.modelData.icon
                                    color: mediaCard.modelData.tint
                                }
                                RoundedImage {
                                    id: thumb
                                    anchors.fill: parent
                                    radius: Theme.rowRadius
                                    visible: mediaCard.hasItem && mediaCard.thumbSource !== ""
                                    source: mediaCard.thumbSource
                                }
                                // Keeps the labels readable over a picture.
                                Rectangle {
                                    visible: thumb.visible
                                    anchors.left: parent.left
                                    anchors.right: parent.right
                                    anchors.bottom: parent.bottom
                                    height: 52
                                    gradient: Gradient {
                                        GradientStop { position: 0.0; color: Qt.rgba(Theme.surface.r, Theme.surface.g, Theme.surface.b, 0) }
                                        GradientStop { position: 1.0; color: Qt.rgba(Theme.surface.r, Theme.surface.g, Theme.surface.b, 0.86) }
                                    }
                                }

                                MouseArea {
                                    id: mediaDragArea
                                    anchors.fill: parent
                                    enabled: mediaCard.hasItem
                                    hoverEnabled: true
                                    cursorShape: enabled ? Qt.OpenHandCursor : Qt.ArrowCursor

                                    property real pressX: 0
                                    property real pressY: 0
                                    property real lastY: 0
                                    property double lastTime: 0
                                    property real velY: 0
                                    property bool dragged: false
                                    property bool flicking: false

                                    Accessible.role: Accessible.Button
                                    Accessible.name: mediaCard.hasItem
                                        ? (mediaCard.modelData.title + ": " + mediaCard.itemData.name)
                                        : mediaCard.modelData.title
                                    Accessible.onPressAction: {
                                        if (mediaCard.hasItem)
                                            AppController.activateShelfItem(mediaCard.modelData.slotKind, "open", mediaCard.targetRef)
                                    }

                                    onPressed: (mouse) => {
                                        pressX = mouse.x
                                        pressY = mouse.y
                                        lastY = mouse.y
                                        lastTime = Date.now()
                                        velY = 0
                                        dragged = false
                                        flicking = false
                                    }

                                    onPositionChanged: (mouse) => {
                                        if (!pressed || dragged || !mediaCard.hasItem)
                                            return
                                        const dx = mouse.x - pressX
                                        const dy = mouse.y - pressY
                                        const now = Date.now()
                                        const dt = Math.max(1, now - lastTime)
                                        velY = ((mouse.y - lastY) * 1000.0) / dt
                                        lastY = mouse.y
                                        lastTime = now

                                        if (dy < -8 && Math.abs(dy) >= Math.abs(dx) * 0.75) {
                                            flicking = true
                                            mediaCard.flickOffsetX = dx * 0.2
                                            mediaCard.flickOffsetY = Math.max(-68, dy * 0.65)
                                        } else if (!flicking && (dx * dx + dy * dy > 64)) {
                                            dragged = true
                                            mediaCard.flickOffsetX = 0
                                            mediaCard.flickOffsetY = 0
                                            AppController.startShelfDrag(
                                                mediaCard.modelData.slotKind,
                                                mediaCard.targetRef,
                                                mediaCard.itemData.name
                                            )
                                        }
                                    }

                                    onReleased: (mouse) => {
                                        if (!flicking)
                                            return
                                        const dy = mouse.y - pressY
                                        const shouldSend = (dy < -24 && velY < -280) || (dy < -48)
                                        flicking = false
                                        dragged = true
                                        mediaCard.flickOffsetX = 0
                                        if (shouldSend && mediaCard.sendUrl.length > 0) {
                                            mediaFlickAnim.restart()
                                            shelfWin.queueSendToPhone([mediaCard.sendUrl], "", mediaCard.itemData.name)
                                        } else {
                                            mediaCard.flickOffsetY = 0
                                        }
                                    }

                                    onCanceled: {
                                        flicking = false
                                        mediaCard.flickOffsetX = 0
                                        mediaCard.flickOffsetY = 0
                                    }

                                    onClicked: {
                                        if (!dragged && mediaCard.hasItem) {
                                            AppController.activateShelfItem(
                                                mediaCard.modelData.slotKind,
                                                "open",
                                                mediaCard.targetRef
                                            )
                                        }
                                    }
                                }

                                // The tile's name and how long ago, bottom left.
                                Column {
                                    anchors.left: parent.left
                                    anchors.right: parent.right
                                    anchors.bottom: parent.bottom
                                    anchors.margins: 8
                                    Txt {
                                        width: parent.width
                                        text: mediaCard.modelData.title
                                        role: "bodySmall"
                                    }
                                    Txt {
                                        width: parent.width
                                        text: mediaCard.hasItem ? shelfWin.relativeTime(mediaCard.itemData.date) : mediaCard.modelData.emptyText
                                        role: "bodySmall"
                                        color: mediaCard.hasItem ? mediaCard.modelData.tint : Theme.surfaceContentVariant
                                    }
                                }

                                // Copy and Open, top right, under the pointer.
                                Row {
                                    anchors.top: parent.top
                                    anchors.right: parent.right
                                    anchors.margins: 4
                                    spacing: 2
                                    visible: mediaCard.hasItem && (mediaDragArea.containsMouse || copyBtn.hovered || openBtn.hovered)
                                    ShelfIcon {
                                        id: copyBtn
                                        iconPath: Icons.copy
                                        label: qsTr("Copy")
                                        onActivated: AppController.activateShelfItem(mediaCard.modelData.slotKind, "copy", mediaCard.targetRef)
                                    }
                                    ShelfIcon {
                                        id: openBtn
                                        iconPath: Icons.openExternal
                                        label: qsTr("Open")
                                        onActivated: AppController.activateShelfItem(mediaCard.modelData.slotKind, "open", mediaCard.targetRef)
                                    }
                                }
                            }
                        }
                    }

                    // ---- Clipboard ----
                    Txt {
                        width: parent.width
                        height: 26
                        verticalAlignment: Text.AlignBottom
                        text: qsTr("Clipboard")
                        role: "label"
                        muted: true
                    }
                    Rectangle {
                        id: clipCard
                        readonly property var clipData: shelfWin.shelfData.currentClip
                        readonly property bool hasClip: clipData !== null && clipData !== undefined
                        readonly property string clipId: hasClip ? (clipData.id || "") : ""
                        readonly property bool isImage: hasClip && clipData.kind === "image"

                        property real flickOffsetX: 0
                        property real flickOffsetY: 0

                        width: parent.width
                        height: isImage ? 76 : Theme.rowHeight
                        radius: Theme.rowRadius
                        color: clipDragArea.containsMouse ? Theme.selection : Theme.raised
                        border.width: clipDragArea.flicking ? 2 : 0
                        border.color: Theme.accent
                        z: (flickOffsetY !== 0 || clipDragArea.flicking) ? 10 : 0

                        transform: Translate {
                            x: clipCard.flickOffsetX
                            y: clipCard.flickOffsetY
                        }

                        Behavior on flickOffsetX {
                            enabled: !Theme.reduceMotion && !clipDragArea.pressed
                            SpringAnimation { spring: 4.5; damping: 0.35; epsilon: 0.25 }
                        }

                        Behavior on flickOffsetY {
                            enabled: !Theme.reduceMotion && !clipDragArea.pressed && !clipFlickAnim.running
                            SpringAnimation { spring: 4.5; damping: 0.35; epsilon: 0.25 }
                        }

                        SequentialAnimation {
                            id: clipFlickAnim
                            NumberAnimation {
                                target: clipCard
                                property: "flickOffsetY"
                                to: -54
                                duration: Theme.reduceMotion ? 0 : 140
                                easing.type: Easing.OutBack
                            }
                            SpringAnimation {
                                target: clipCard
                                property: "flickOffsetY"
                                to: 0
                                spring: 4.5
                                damping: 0.35
                                epsilon: 0.25
                            }
                        }

                        Icon {
                            id: clipGlyph
                            x: 8
                            anchors.verticalCenter: parent.verticalCenter
                            width: 16; height: 16
                            path: Icons.clipboard
                            color: Theme.tintLavender
                        }
                        RoundedImage {
                            anchors.left: clipGlyph.right
                            anchors.leftMargin: 10
                            anchors.right: clipActions.left
                            anchors.rightMargin: 8
                            anchors.top: parent.top
                            anchors.bottom: parent.bottom
                            anchors.margins: 6
                            radius: Theme.radiusXs
                            fillMode: Image.PreserveAspectFit
                            visible: clipCard.isImage
                            source: clipCard.isImage && clipCard.clipData.imageDataUrl ? clipCard.clipData.imageDataUrl : ""
                        }
                        Txt {
                            anchors.left: clipGlyph.right
                            anchors.leftMargin: 10
                            anchors.right: clipActions.left
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            visible: !clipCard.isImage
                            text: clipCard.hasClip ? String(clipCard.clipData.text || "").replace(/\s+/g, " ") : qsTr("Empty")
                            role: "bodySmall"
                            muted: !clipCard.hasClip
                        }

                        MouseArea {
                            id: clipDragArea
                            anchors.fill: parent
                            enabled: clipCard.hasClip
                            hoverEnabled: true
                            cursorShape: enabled ? Qt.OpenHandCursor : Qt.ArrowCursor

                            property real pressX: 0
                            property real pressY: 0
                            property real lastY: 0
                            property double lastTime: 0
                            property real velY: 0
                            property bool dragged: false
                            property bool flicking: false

                            Accessible.role: Accessible.Button
                            Accessible.name: clipCard.hasClip
                                ? qsTr("Current clip: %1").arg(clipCard.clipData.text)
                                : qsTr("Clipboard is empty")
                            Accessible.onPressAction: {
                                if (clipCard.hasClip)
                                    AppController.activateShelfItem("clip", "copy", clipCard.clipId)
                            }

                            onPressed: (mouse) => {
                                pressX = mouse.x
                                pressY = mouse.y
                                lastY = mouse.y
                                lastTime = Date.now()
                                velY = 0
                                dragged = false
                                flicking = false
                            }

                            onPositionChanged: (mouse) => {
                                if (!pressed || dragged || !clipCard.hasClip)
                                    return
                                const dx = mouse.x - pressX
                                const dy = mouse.y - pressY
                                const now = Date.now()
                                const dt = Math.max(1, now - lastTime)
                                velY = ((mouse.y - lastY) * 1000.0) / dt
                                lastY = mouse.y
                                lastTime = now

                                if (dy < -8 && Math.abs(dy) >= Math.abs(dx) * 0.75) {
                                    flicking = true
                                    clipCard.flickOffsetX = dx * 0.2
                                    clipCard.flickOffsetY = Math.max(-68, dy * 0.65)
                                } else if (!flicking && (dx * dx + dy * dy > 64)) {
                                    dragged = true
                                    clipCard.flickOffsetX = 0
                                    clipCard.flickOffsetY = 0
                                    AppController.startShelfDrag(
                                        "clip",
                                        clipCard.clipId,
                                        clipCard.clipData.text || ""
                                    )
                                }
                            }

                            onReleased: (mouse) => {
                                if (!flicking)
                                    return
                                const dy = mouse.y - pressY
                                const shouldSend = (dy < -24 && velY < -280) || (dy < -48)
                                flicking = false
                                dragged = true
                                clipCard.flickOffsetX = 0
                                if (shouldSend && clipCard.isImage && clipCard.clipId.length > 0) {
                                    clipFlickAnim.restart()
                                    shelfWin.queueSendToPhone([], "", qsTr("the copied image"), clipCard.clipId)
                                } else if (shouldSend && (clipCard.clipData.text || "").length > 0) {
                                    clipFlickAnim.restart()
                                    shelfWin.queueSendToPhone([], clipCard.clipData.text || "", qsTr("the copied text"))
                                } else {
                                    clipCard.flickOffsetY = 0
                                }
                            }

                            onCanceled: {
                                flicking = false
                                clipCard.flickOffsetX = 0
                                clipCard.flickOffsetY = 0
                            }

                            onClicked: {
                                if (!dragged && clipCard.hasClip) {
                                    AppController.activateShelfItem("clip", "copy", clipCard.clipId)
                                }
                            }
                        }

                        Row {
                            id: clipActions
                            anchors.right: parent.right
                            anchors.rightMargin: 4
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            visible: clipCard.hasClip
                            ShelfIcon {
                                visible: clipCard.hasClip
                                    && ((clipCard.clipData.text || "").length > 0 || (clipCard.isImage && clipCard.clipId.length > 0))
                                iconPath: Icons.send
                                label: qsTr("Send to phone")
                                onActivated: {
                                    clipFlickAnim.restart()
                                    if (clipCard.isImage)
                                        shelfWin.queueSendToPhone([], "", qsTr("the copied image"), clipCard.clipId)
                                    else
                                        shelfWin.queueSendToPhone([], clipCard.clipData.text || "", qsTr("the copied text"))
                                }
                            }
                            ShelfIcon {
                                iconPath: Icons.copy
                                label: qsTr("Copy")
                                onActivated: AppController.activateShelfItem("clip", "copy", clipCard.clipId)
                            }
                        }
                    }

                    // ---- Received files ----
                    Txt {
                        width: parent.width
                        height: 26
                        verticalAlignment: Text.AlignBottom
                        text: qsTr("Received")
                        role: "label"
                        muted: true
                    }
                    Rectangle {
                        width: parent.width
                        height: Theme.rowHeight
                        radius: Theme.rowRadius
                        color: Theme.raised
                        visible: (shelfWin.shelfData.recentFiles || []).length === 0
                        Txt {
                            x: 10
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Nothing yet")
                            role: "bodySmall"
                            muted: true
                        }
                    }
                    Repeater {
                        model: shelfWin.shelfData.recentFiles || []

                        delegate: Rectangle {
                            id: fileRowCard
                            required property var modelData
                            width: cardsColumn.width
                            height: Theme.rowHeight
                            radius: Theme.rowRadius
                            color: fileHover.hovered ? Theme.selection : Theme.raised
                            border.width: fileRowDrag.flicking ? 2 : 0
                            border.color: Theme.accent
                            z: (flickOffsetY !== 0 || fileRowDrag.flicking) ? 10 : 0

                            property real flickOffsetX: 0
                            property real flickOffsetY: 0

                            HoverHandler { id: fileHover }

                            transform: Translate {
                                x: fileRowCard.flickOffsetX
                                y: fileRowCard.flickOffsetY
                            }

                            Behavior on flickOffsetX {
                                enabled: !Theme.reduceMotion && !fileRowDrag.pressed
                                SpringAnimation { spring: 4.5; damping: 0.35; epsilon: 0.25 }
                            }

                            Behavior on flickOffsetY {
                                enabled: !Theme.reduceMotion && !fileRowDrag.pressed && !fileFlickAnim.running
                                SpringAnimation { spring: 4.5; damping: 0.35; epsilon: 0.25 }
                            }

                            SequentialAnimation {
                                id: fileFlickAnim
                                NumberAnimation {
                                    target: fileRowCard
                                    property: "flickOffsetY"
                                    to: -54
                                    duration: Theme.reduceMotion ? 0 : 140
                                    easing.type: Easing.OutBack
                                }
                                SpringAnimation {
                                    target: fileRowCard
                                    property: "flickOffsetY"
                                    to: 0
                                    spring: 4.5
                                    damping: 0.35
                                    epsilon: 0.25
                                }
                            }

                            MouseArea {
                                id: fileRowDrag
                                anchors.left: parent.left
                                anchors.top: parent.top
                                anchors.bottom: parent.bottom
                                anchors.right: fileActions.visible ? fileActions.left : parent.right
                                hoverEnabled: true
                                cursorShape: Qt.OpenHandCursor

                                property real pressX: 0
                                property real pressY: 0
                                property real lastY: 0
                                property double lastTime: 0
                                property real velY: 0
                                property bool dragged: false
                                property bool flicking: false

                                Accessible.role: Accessible.Button
                                Accessible.name: fileRowCard.modelData.name
                                Accessible.onPressAction: AppController.activateShelfItem("file", "open", fileRowCard.modelData.path)

                                onPressed: (mouse) => {
                                    pressX = mouse.x
                                    pressY = mouse.y
                                    lastY = mouse.y
                                    lastTime = Date.now()
                                    velY = 0
                                    dragged = false
                                    flicking = false
                                }

                                onPositionChanged: (mouse) => {
                                    if (!pressed || dragged)
                                        return
                                    const dx = mouse.x - pressX
                                    const dy = mouse.y - pressY
                                    const now = Date.now()
                                    const dt = Math.max(1, now - lastTime)
                                    velY = ((mouse.y - lastY) * 1000.0) / dt
                                    lastY = mouse.y
                                    lastTime = now

                                    if (dy < -8 && Math.abs(dy) >= Math.abs(dx) * 0.75) {
                                        flicking = true
                                        fileRowCard.flickOffsetX = dx * 0.2
                                        fileRowCard.flickOffsetY = Math.max(-68, dy * 0.65)
                                    } else if (!flicking && (dx * dx + dy * dy > 64)) {
                                        dragged = true
                                        fileRowCard.flickOffsetX = 0
                                        fileRowCard.flickOffsetY = 0
                                        AppController.startShelfDrag("file", fileRowCard.modelData.path, fileRowCard.modelData.name)
                                    }
                                }

                                onReleased: (mouse) => {
                                    if (!flicking)
                                        return
                                    const dy = mouse.y - pressY
                                    const shouldSend = (dy < -24 && velY < -280) || (dy < -48)
                                    flicking = false
                                    dragged = true
                                    fileRowCard.flickOffsetX = 0
                                    if (shouldSend && fileRowCard.modelData.path && fileRowCard.modelData.path.length > 0) {
                                        fileFlickAnim.restart()
                                        shelfWin.queueSendToPhone([fileRowCard.modelData.path], "", fileRowCard.modelData.name)
                                    } else {
                                        fileRowCard.flickOffsetY = 0
                                    }
                                }

                                onCanceled: {
                                    flicking = false
                                    fileRowCard.flickOffsetX = 0
                                    fileRowCard.flickOffsetY = 0
                                }

                                onClicked: {
                                    if (!dragged)
                                        AppController.activateShelfItem("file", "open", fileRowCard.modelData.path)
                                }
                            }

                            Icon {
                                id: fileGlyph
                                x: 8
                                anchors.verticalCenter: parent.verticalCenter
                                width: 16; height: 16
                                path: Icons.folder
                                color: Theme.tintAmber
                            }
                            Txt {
                                id: fileName
                                anchors.left: fileGlyph.right
                                anchors.leftMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.min(implicitWidth, (fileActions.visible ? fileActions.x : parent.width - 8) - x - 8 - Math.min(fileSize.implicitWidth + 8, 70))
                                text: fileRowCard.modelData.name
                                role: "bodySmall"
                            }
                            Txt {
                                id: fileSize
                                anchors.left: fileName.right
                                anchors.leftMargin: 8
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.max(0, (fileActions.visible ? fileActions.x : parent.width - 8) - x - 8)
                                text: shelfWin.formatBytes(fileRowCard.modelData.size)
                                role: "bodySmall"
                                muted: true
                            }

                            // Send, Open and Show in folder, under the pointer.
                            Row {
                                id: fileActions
                                anchors.right: parent.right
                                anchors.rightMargin: 4
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 2
                                visible: fileHover.hovered

                                ShelfIcon {
                                    iconPath: Icons.send
                                    label: qsTr("Send to phone")
                                    onActivated: {
                                        fileFlickAnim.restart()
                                        shelfWin.queueSendToPhone([fileRowCard.modelData.path], "", fileRowCard.modelData.name)
                                    }
                                }
                                ShelfIcon {
                                    iconPath: Icons.openExternal
                                    label: qsTr("Open")
                                    onActivated: AppController.activateShelfItem("file", "open", fileRowCard.modelData.path)
                                }
                                ShelfIcon {
                                    iconPath: Icons.folder
                                    label: qsTr("Show in folder")
                                    onActivated: AppController.activateShelfItem("file", "reveal", fileRowCard.modelData.path)
                                }
                            }
                        }
                    }
                }
            }

            // ---- The drop zone, which becomes the 3-second "Sending… Undo" bar ----
            Item {
                id: dropZone
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.margins: 14
                height: 60

                readonly property bool lit: shelfDropArea.containsDrag || shelfWin.undoPending

                Accessible.role: Accessible.StaticText
                Accessible.name: shelfWin.undoPending ? undoText.text : dropLabel.text

                Rectangle {
                    anchors.fill: parent
                    radius: 10
                    color: dropZone.lit ? Theme.raised : "transparent"
                    Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
                }
                Shape {
                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    ShapePath {
                        strokeColor: dropZone.lit ? Theme.accent : Theme.outline
                        strokeWidth: 1
                        strokeStyle: dropZone.lit ? ShapePath.SolidLine : ShapePath.DashLine
                        dashPattern: [4, 3]
                        fillColor: "transparent"
                        PathRectangle {
                            x: 0.5; y: 0.5
                            width: dropZone.width - 1
                            height: dropZone.height - 1
                            radius: 10
                        }
                    }
                }

                // Waiting to drop, just dropped, or cancelled.
                Column {
                    anchors.centerIn: parent
                    width: parent.width - 24
                    visible: !shelfWin.undoPending
                    spacing: 2
                    Txt {
                        id: dropLabel
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        role: "bodySmall"
                        color: shelfDropArea.containsDrag || shelfWin.justDropped ? Theme.surfaceContent : Theme.surfaceContentVariant
                        text: shelfDropArea.containsDrag
                            ? qsTr("Release to send to %1").arg(shelfWin.targetPhoneName)
                            : (shelfWin.justDropped
                                ? (shelfWin.shelfData.online
                                    ? qsTr("Sent to %1").arg(shelfWin.targetPhoneName)
                                    : qsTr("Queued for %1").arg(shelfWin.targetPhoneName))
                                : (shelfWin.undoCancelled
                                    ? qsTr("Send cancelled")
                                    : qsTr("Drop files here to send them to the phone")))
                    }
                    Txt {
                        width: parent.width
                        visible: !shelfDropArea.containsDrag && !shelfWin.justDropped && !shelfWin.undoCancelled
                        horizontalAlignment: Text.AlignHCenter
                        role: "caption"
                        muted: true
                        text: qsTr("Flick a card up to send it, or drag it out")
                    }
                }

                // "Sending to <phone>…" with Undo, and the countdown along the bottom.
                Item {
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 8
                    visible: shelfWin.undoPending

                    Icon {
                        id: undoGlyph
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16; height: 16
                        path: Icons.send
                        color: Theme.tintAmber
                    }
                    Txt {
                        id: undoText
                        anchors.left: undoGlyph.right
                        anchors.leftMargin: 10
                        anchors.right: undoBtn.left
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        role: "bodySmall"
                        text: shelfWin.pendingLabel.length > 0
                            ? qsTr("Sending %1 to %2…").arg(shelfWin.pendingLabel).arg(shelfWin.targetPhoneName)
                            : qsTr("Sending to %1…").arg(shelfWin.targetPhoneName)
                    }
                    Button {
                        id: undoBtn
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        variant: "tonal"
                        size: "sm"
                        text: qsTr("Undo")
                        onClicked: shelfWin.cancelPendingSend()
                    }
                }
                Rectangle {
                    x: 10
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 3
                    height: 2
                    radius: 1
                    width: (parent.width - 20) * Math.max(0, Math.min(1, shelfWin.undoProgress))
                    color: Theme.accent
                    visible: shelfWin.undoPending
                }
            }
        }
    }

    // A small bare-glyph button on a card or row.
    component ShelfIcon: Rectangle {
        id: si
        property string iconPath
        property string label
        readonly property bool hovered: siMouse.containsMouse
        signal activated
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        width: 24; height: 24
        radius: Theme.radiusSm
        color: siMouse.containsMouse ? Theme.selection : Qt.rgba(Theme.surface.r, Theme.surface.g, Theme.surface.b, 0.72)
        Accessible.role: Accessible.Button
        Accessible.name: label
        Icon {
            anchors.centerIn: parent
            width: 14; height: 14
            path: si.iconPath
            color: Theme.surfaceContent
        }
        MouseArea {
            id: siMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: si.activated()
        }
    }
}
