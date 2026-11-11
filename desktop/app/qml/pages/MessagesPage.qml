// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Dialogs
import app.g7link

// Messages: the phone's SMS and mirrored chat-app conversations on the left,
// the open one on the right, and a box to text, attach an MMS image or voice
// note, or reply inline from the PC (sent through the phone).
Item {
    id: page
    property bool active: true
    // The phone shown (the one on Home).
    property string deviceId
    property string deviceName
    // Writing to someone new (no conversation open).
    property bool composing: false

    readonly property var threads: { try { return JSON.parse(Messages.threads) } catch (e) { return [] } }
    readonly property var messages: { try { return JSON.parse(Messages.messages) } catch (e) { return [] } }
    readonly property var contacts: { try { return JSON.parse(PhoneCall.contacts) } catch (e) { return [] } }
    readonly property var availableApps: { try { return JSON.parse(Messages.availableApps) } catch (e) { return [] } }
    readonly property var openThread: threads.find(t => t.id === Messages.thread) || null
    readonly property bool ready: Messages.status === "ready"
    // Conversations matching the search box.
    readonly property string query: search.text.trim().toLocaleLowerCase()
    readonly property var shownThreads: query.length === 0 ? threads
        : threads.filter(t => (t.title + " " + t.addresses + " " + (t.appName || "") + " " + (t.lastSender || "") + " " + t.snippet).toLocaleLowerCase().indexOf(query) >= 0)

    // Contacts matching the "To" field when starting a new message.
    readonly property string recipientQuery: toField.text.trim().toLocaleLowerCase()
    readonly property var recipientMatches: {
        if (!composing || recipientQuery.length === 0) return []
        const out = []
        for (let i = 0; i < contacts.length && out.length < 6; i++) {
            const c = contacts[i]
            const nums = c.numbers || []
            for (let j = 0; j < nums.length && out.length < 6; j++) {
                const num = nums[j].number || ""
                const label = nums[j].label || ""
                // Don't show a suggestion if its number is already selected verbatim.
                if (num === toField.text.trim()) continue
                if ((c.name + " " + num + " " + label).toLocaleLowerCase().indexOf(recipientQuery) >= 0) {
                    out.push({
                        name: c.name,
                        number: num,
                        label: label,
                        photo: c.photo || ""
                    })
                }
            }
        }
        return out
    }

    // ---- Footer and Actions (Ctrl B) ----
    readonly property bool chatOpen: !!openThread || composing
    readonly property bool canCallThread: !!openThread && !openThread.isChat && !openThread.group
        && openThread.addresses.length > 0 && PhoneCall.canDial
    readonly property string footerLabel: unreadCount > 0 ? qsTr("Messages · %1 unread").arg(unreadCount) : qsTr("Messages")
    readonly property int unreadCount: threads.reduce((sum, t) => sum + (t.unread > 0 ? t.unread : 0), 0)
    readonly property var primaryAction: chatOpen
        ? ({ label: qsTr("Send"), keys: "Enter", run: () => composer.send() })
        : ({ label: qsTr("New message"), keys: "Ctrl N", run: () => page.startNew() })
    readonly property var screenActions: {
        const list = [
            { label: qsTr("Send"), icon: Icons.send, tint: Theme.accent, keys: "Enter",
              enabled: page.chatOpen && composer.canSend, run: () => composer.send() },
            { label: qsTr("New message"), icon: Icons.plus, tint: Theme.accent, keys: "Ctrl N",
              enabled: page.ready, run: () => page.startNew() },
            { label: qsTr("Search conversations"), icon: Icons.search, keys: "Ctrl F",
              enabled: page.ready, run: () => search.input.forceActiveFocus() },
            { label: qsTr("Attach image or voice note"), icon: Icons.photo, tint: Theme.tintCoral,
              enabled: page.chatOpen && !composer.isChat && !composer.groupSms && page.ready && !Messages.sending,
              run: () => imageFileDialog.open() },
            { label: qsTr("Paste image from clipboard"), icon: Icons.clipboard, tint: Theme.tintLavender, keys: "Ctrl V",
              enabled: page.chatOpen && !composer.isChat && !composer.groupSms,
              run: () => { if (Messages.pasteClipboardImage()) composer.focusText() } },
            { label: qsTr("Remove attachment"), icon: Icons.close, enabled: Messages.hasAttachment,
              run: () => Messages.clearAttachment() },
            { label: page.openThread ? qsTr("Call %1").arg(page.openThread.title) : qsTr("Call"), icon: Icons.call,
              tint: Theme.tintTeal, enabled: page.canCallThread && !PhoneCall.dialing,
              run: () => { if (page.openThread) PhoneCall.dial(page.openThread.addresses) } },
            { label: qsTr("Remove conversation from PC"), icon: Icons.trash, tint: Theme.tintRed,
              enabled: !!page.openThread && !!page.openThread.isChat,
              run: () => { if (page.openThread) Messages.deleteChatThread(page.openThread.id) } },
            { label: qsTr("Close conversation"), icon: Icons.close, keys: "Esc", enabled: page.chatOpen,
              run: () => page.closeChat() },
            { label: qsTr("Load older messages"), icon: Icons.history, enabled: !!page.openThread && Messages.more
              && !Messages.loadingOlder, run: () => Messages.loadOlder() },
            { label: qsTr("Refresh"), icon: Icons.refresh, enabled: Messages.status !== "loading",
              run: () => Messages.refresh() }
        ]
        if (Messages.hasChatThreads && page.availableApps.length > 2)
            for (const app of page.availableApps)
                list.push({ label: qsTr("Show %1").arg(app.label), icon: Icons.messages, tint: Theme.tintBlue,
                            checked: Messages.appFilter === app.key, run: () => Messages.filterByApp(app.key) })
        return list
    }

    FileDialog {
        id: imageFileDialog
        title: qsTr("Attach image or voice note")
        nameFilters: [
            qsTr("Media (*.png *.jpg *.jpeg *.webp *.gif *.bmp *.m4a *.mp3 *.aac *.ogg *.wav *.amr *.3gp)"),
            qsTr("Images (*.png *.jpg *.jpeg *.webp *.gif *.bmp)"),
            qsTr("Audio (*.m4a *.mp3 *.aac *.ogg *.wav *.amr *.3gp)")
        ]
        onAccepted: {
            if (selectedFile.toString().length > 0) {
                Messages.attachFile(selectedFile.toString())
                composer.focusText()
            }
        }
    }

    // Back to the list: nothing open.
    function closeChat() {
        Messages.closeThread()
        composing = false
        toField.text = ""
    }

    // A stable color for a contact (in Bloom).
    function hueOf(name) {
        let h = 0
        for (let i = 0; i < name.length; i++)
            h = (h * 31 + name.charCodeAt(i)) % 360
        return h / 360
    }

    // "Today", "Yesterday", or the date, for separators.
    function dayLabel(ms) {
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

    // Text as HTML with its web links clickable.
    function linkified(text) {
        const escaped = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/\n/g, "<br>")
        return escaped.replace(/\b(https?:\/\/[^\s<]+[^\s<.,;:!?)\]'"])/g, '<a href="$1">$1</a>')
    }

    // How many texts a message takes (GSM letters: 160 in one, 153 each
    // when split; anything else: 70 and 67).
    function smsCount(text) {
        const gsm = /^[\x0A\x0D\x20-\x7E£¥èéùìòÇØøÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉÄÖÑÜ§¿äöñüà€]*$/.test(text)
        const one = gsm ? 160 : 70
        const each = gsm ? 153 : 67
        return text.length <= one ? 1 : Math.ceil(text.length / each)
    }

    visible: active

    function applyComposeTarget() {
        if (Messages.composeTo.length > 0) {
            composing = true
            toField.text = Messages.composeTo
            Qt.callLater(() => composer.focusText())
        } else if (Messages.thread.length > 0) {
            composing = false
        }
    }

    Connections {
        target: Messages
        function onComposeToChanged() { page.applyComposeTarget() }
        function onThreadChanged() {
            page.switchDraft()
            if (Messages.thread.length > 0) {
                page.composing = false
                Qt.callLater(() => composer.focusText())
            }
        }
    }

    function load() {
        if (active && deviceId.length > 0) {
            Messages.open(deviceId)
            PhoneCall.ensureContacts(deviceId)
            applyComposeTarget()
        }
    }
    // Leaving the page closes the open conversation.
    onActiveChanged: active ? load() : closeChat()
    Component.onCompleted: load()
    Component.onDestruction: Messages.closeThread()

    Shortcut {
        sequence: "Esc"
        enabled: page.active && (!!page.openThread || page.composing)
        onActivated: page.closeChat()
    }
    Shortcut {
        sequences: [StandardKey.Find]
        enabled: page.active && page.ready
        onActivated: search.input.forceActiveFocus()
    }
    Shortcut {
        sequences: [StandardKey.New]
        enabled: page.active && page.ready
        onActivated: page.startNew()
    }
    Shortcut {
        sequences: [StandardKey.Paste]
        enabled: page.active && (!!page.openThread || page.composing) && !composer.isChat && !composer.groupSms && !input.activeFocus
        onActivated: {
            if (Messages.pasteClipboardImage()) {
                composer.focusText()
            }
        }
    }
    function startNew() {
        Messages.closeThread()
        composing = true
        toField.text = ""
        toField.forceActiveFocus()
    }
    onDeviceIdChanged: { composing = false; switchDraft(); load() }

    // What was typed in each conversation, so text written for one is never
    // sent to another and comes back when you return to it.
    property var drafts: ({})
    property string draftKey: ""
    function switchDraft() {
        const key = deviceId + "|" + Messages.thread
        if (key === draftKey)
            return
        if (draftKey.length > 0) {
            if (input.text.length > 0)
                drafts[draftKey] = input.text
            else
                delete drafts[draftKey]
        }
        draftKey = key
        input.text = drafts[key] || ""
    }

    // "14:05", "Yesterday", "Tue", or a date.
    function shortTime(ms) {
        const date = new Date(ms)
        const now = new Date()
        if (date.toDateString() === now.toDateString())
            return date.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
        const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1)
        if (date.toDateString() === yesterday.toDateString())
            return qsTr("Yesterday")
        if (now - date < 6 * 24 * 3600 * 1000)
            return date.toLocaleDateString(Qt.locale(), "ddd")
        return date.toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }

    // ---- No phone, or messages not available ----
    Column {
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 380)
        spacing: 8
        visible: !list.visible && (page.deviceId.length === 0 || Messages.status !== "loading")
        Icon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 28; height: 28
            path: Icons.messages
            color: Theme.tintBlue
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            weight: 500
            text: page.deviceId.length === 0 ? qsTr("No phone paired")
                : Messages.status === "offline" ? qsTr("%1 isn't connected").arg(page.deviceName)
                : Messages.status === "off" ? qsTr("Messages are off for %1").arg(page.deviceName)
                : Messages.status === "unsupported" ? qsTr("Allow texting on %1").arg(page.deviceName)
                : qsTr("Couldn't load messages")
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            visible: page.deviceId.length > 0 && (Messages.status === "off" || Messages.status === "unsupported")
            role: "bodySmall"
            muted: true
            text: Messages.status === "off" ? qsTr("Turn on Messages in Settings")
                : qsTr("Grant SMS permission on %1").arg(page.deviceName)
        }
        Button {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: page.deviceId.length > 0 && Messages.status === "failed"
            variant: "tonal"
            size: "sm"
            text: qsTr("Try again")
            onClicked: page.load()
        }
    }

    Spinner {
        anchors.centerIn: parent
        visible: Messages.status === "loading" && page.threads.length === 0 && !Messages.hasChatThreads
    }

    // ---- Conversations ----
    Item {
        id: list
        visible: page.threads.length > 0 || Messages.hasChatThreads || (page.ready && page.deviceId.length > 0)
        width: Math.min(340, Math.max(280, page.width * 0.34))
        height: parent.height

        SectionLabel {
            id: listHeader
            x: 8
            y: 8
            width: parent.width - 16
            height: 34
            text: qsTr("Conversations")
            actionText: qsTr("New")
            actionIcon: Icons.plus
            enabled: page.ready
            onActionClicked: page.startNew()
        }

        // Search: by name, number, app, or the latest text.
        Field {
            id: search
            anchors.top: listHeader.bottom
            x: 8
            width: parent.width - 16
            height: 28
            iconPath: Icons.search
            placeholder: qsTr("Search")
            Accessible.name: qsTr("Search conversations")
            Keys.onEscapePressed: (event) => {
                event.accepted = search.text.length > 0
                search.text = ""
            }
        }

        // Chat apps, when mirrored chat conversations are here.
        Flickable {
            id: appFilterBar
            anchors.top: search.bottom
            anchors.topMargin: visible ? 6 : 0
            anchors.left: parent.left
            anchors.right: parent.right
            height: visible ? 26 : 0
            visible: Messages.hasChatThreads && page.availableApps.length > 2
            contentWidth: filterRow.implicitWidth + 16
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Row {
                id: filterRow
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4
                Repeater {
                    model: page.availableApps
                    delegate: TextAction {
                        required property var modelData
                        text: modelData.label
                        selected: Messages.appFilter === modelData.key
                        onClicked: Messages.filterByApp(modelData.key)
                    }
                }
            }
        }

        ListView {
            id: threadList
            anchors.top: appFilterBar.bottom
            anchors.topMargin: 6
            anchors.bottom: parent.bottom
            x: 8
            width: parent.width - 16
            clip: true
            spacing: 2
            boundsBehavior: Flickable.StopAtBounds
            model: page.shownThreads
            delegate: ThreadRow {
                required property var modelData
                width: threadList.width
                thread: modelData
            }
            Txt {
                anchors.centerIn: parent
                visible: (page.ready || Messages.hasChatThreads) && page.shownThreads.length === 0
                muted: true
                text: page.query.length > 0 ? qsTr("No matches") : qsTr("No conversations")
            }
        }
        Rectangle {
            anchors.right: parent.right
            width: 1
            height: parent.height
            color: Theme.line
        }
    }

    // ---- The open conversation, or a new one ----
    Item {
        id: chat
        anchors.left: list.right
        anchors.right: parent.right
        height: parent.height
        visible: list.visible

        // An image or voice note dropped on an open SMS conversation.
        DropArea {
            id: mmsDropArea
            anchors.fill: parent
            enabled: (!!page.openThread || page.composing) && !composer.isChat && !composer.groupSms
            onDropped: (drop) => {
                if (drop.hasUrls && drop.urls.length > 0) {
                    if (Messages.attachFile(drop.urls[0].toString())) {
                        drop.acceptProposedAction()
                        composer.focusText()
                    }
                }
            }
        }

        Rectangle {
            anchors.fill: parent
            anchors.margins: 12
            z: 10
            visible: mmsDropArea.containsDrag
            radius: Theme.radiusXl
            color: Qt.rgba(Theme.surface.r, Theme.surface.g, Theme.surface.b, 0.92)
            border.width: 1
            border.color: Theme.accent
            Column {
                anchors.centerIn: parent
                spacing: 4
                Icon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 24; height: 24
                    path: Icons.photo
                    color: Theme.tintCoral
                }
                Txt {
                    anchors.horizontalCenter: parent.horizontalCenter
                    weight: 500
                    text: qsTr("Drop to attach (MMS)")
                }
                Txt {
                    anchors.horizontalCenter: parent.horizontalCenter
                    role: "bodySmall"
                    muted: true
                    text: qsTr("PNG, JPEG, WebP, GIF or BMP, up to 900 KB")
                }
            }
        }

        Txt {
            anchors.centerIn: parent
            visible: !page.openThread && !page.composing && list.visible
            muted: true
            text: qsTr("Pick a conversation")
        }

        Item {
            id: chatHeader
            visible: !!page.openThread || page.composing
            width: parent.width
            height: 52

            // Who: an avatar, the name and, under it, the number and the app.
            Item {
                id: headerFace
                x: 16
                anchors.verticalCenter: parent.verticalCenter
                width: 28; height: 28
                visible: !page.composing
                RoundedImage {
                    anchors.fill: parent
                    radius: width / 2
                    visible: !!page.openThread && (page.openThread.photo || "").length > 0 && status === Image.Ready
                    source: page.openThread ? page.openThread.photo || "" : ""
                    sourceSize: Qt.size(56, 56)
                    fillMode: Image.PreserveAspectCrop
                }
                Avatar {
                    anchors.fill: parent
                    visible: !page.openThread || (page.openThread.photo || "").length === 0
                    readonly property string title: page.openThread ? page.openThread.title : ""
                    readonly property bool named: !/^[\d+(#*]/.test(title)
                    name: page.openThread && !page.openThread.group && named ? title : ""
                    iconPath: page.openThread && page.openThread.group ? Icons.messages : named ? "" : Icons.person
                    hue: named && page.openThread && !page.openThread.group ? page.hueOf(title) : -1
                }
            }
            Column {
                anchors.left: headerFace.right
                anchors.leftMargin: 10
                anchors.right: headerActions.left
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                visible: !page.composing
                Txt {
                    width: parent.width
                    text: page.openThread ? page.openThread.title : ""
                    weight: 500
                }
                Txt {
                    width: parent.width
                    role: "bodySmall"
                    muted: true
                    text: {
                        const t = page.openThread
                        if (!t)
                            return ""
                        const app = t.isChat ? (t.appName || Messages.currentAppName) : "SMS"
                        return !t.isChat && t.title !== t.addresses && t.addresses.length > 0
                            ? t.addresses + " · " + app : app
                    }
                }
            }
            Row {
                id: headerActions
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                visible: !page.composing
                spacing: 2
                IconButton {
                    visible: !!page.openThread && !page.openThread.isChat && !page.openThread.group
                        && page.openThread.addresses.length > 0 && PhoneCall.canDial
                    enabled: !PhoneCall.dialing
                    iconPath: Icons.call
                    iconColor: Theme.tintTeal
                    label: page.openThread ? qsTr("Call %1").arg(page.openThread.title) : qsTr("Call")
                    onClicked: if (page.openThread) PhoneCall.dial(page.openThread.addresses)
                }
                IconButton {
                    visible: !!page.openThread && !!page.openThread.isChat
                    iconPath: Icons.trash
                    iconColor: Theme.tintRed
                    label: qsTr("Remove conversation from PC")
                    onClicked: if (page.openThread) Messages.deleteChatThread(page.openThread.id)
                }
                IconButton {
                    iconPath: Icons.close
                    label: qsTr("Close conversation (Esc)")
                    onClicked: page.closeChat()
                }
            }
            // A new message: who to.
            Item {
                anchors.left: parent.left
                anchors.leftMargin: 16
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                height: 32
                visible: page.composing
                Txt {
                    id: toLabel
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("To")
                    muted: true
                }
                Rectangle {
                    anchors.left: toLabel.right
                    anchors.leftMargin: 10
                    anchors.right: cancelNew.left
                    anchors.rightMargin: 8
                    height: parent.height
                    radius: Theme.rowRadius
                    color: Theme.raised
                    border.width: toField.activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                    TextInput {
                        id: toField
                        anchors.left: parent.left
                        anchors.leftMargin: 10
                        anchors.right: parent.right
                        anchors.rightMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        font.family: Theme.fontUi
                        font.pixelSize: Theme.typography.body.size
                        color: Theme.surfaceContent
                        selectionColor: Theme.selection
                        selectedTextColor: Theme.surfaceContent
                        clip: true
                        Accessible.name: qsTr("Name or phone number")
                        Keys.onReturnPressed: {
                            if (page.recipientMatches.length > 0 && !/\d/.test(toField.text)) {
                                toField.text = page.recipientMatches[0].number
                            }
                            composer.focusText()
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            visible: toField.text.length === 0
                            muted: true
                            text: qsTr("Name or phone number")
                        }
                    }
                }
                IconButton {
                    id: cancelNew
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: Icons.close
                    label: qsTr("Cancel (Esc)")
                    onClicked: page.closeChat()
                }
            }
            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: 1
                color: Theme.line
            }
        }

        // Contacts matching what's typed in "To".
        Rectangle {
            id: contactSuggestions
            z: 2
            anchors.top: chatHeader.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: 16
            anchors.rightMargin: 16
            anchors.topMargin: 6
            visible: page.composing && page.recipientMatches.length > 0
            height: suggestionColumn.height + 12
            radius: Theme.radiusXl
            color: Theme.surface
            border.width: 1
            border.color: Theme.line

            Column {
                id: suggestionColumn
                x: 6
                y: 6
                width: parent.width - 12
                Repeater {
                    model: page.recipientMatches
                    delegate: ActionRow {
                        id: srow
                        required property var modelData
                        width: suggestionColumn.width
                        title: modelData.name
                        detail: modelData.label.length > 0 ? modelData.label + " · " + modelData.number : modelData.number
                        accessibleName: modelData.name + ", " + modelData.number
                        onClicked: {
                            toField.text = modelData.number
                            composer.focusText()
                        }
                        leading: Item {
                            width: 24; height: 24
                            RoundedImage {
                                anchors.fill: parent
                                radius: width / 2
                                visible: srow.modelData.photo.length > 0 && status === Image.Ready
                                source: srow.modelData.photo
                                sourceSize: Qt.size(48, 48)
                                fillMode: Image.PreserveAspectCrop
                            }
                            Avatar {
                                anchors.fill: parent
                                visible: srow.modelData.photo.length === 0
                                name: srow.modelData.name
                                hue: page.hueOf(srow.modelData.name)
                            }
                        }
                    }
                }
            }
        }

        ListView {
            id: messageList
            anchors.top: chatHeader.bottom
            anchors.bottom: attachmentBar.visible ? attachmentBar.top : composerBar.top
            anchors.bottomMargin: 8
            width: parent.width
            visible: !!page.openThread
            clip: true
            spacing: 8
            cacheBuffer: 960
            // A short conversation sits at the bottom, by the box.
            topMargin: Math.max(16, height - contentHeight - bottomMargin)
            bottomMargin: 8
            boundsBehavior: Flickable.StopAtBounds
            model: page.messages
            // The latest at the bottom; scrolling up loads older ones. Every refresh
            // replaces the whole list, which puts a list view back at the top, and
            // pictures grow a bubble once loaded: stay where the reader was. By the
            // message at the top of the view, since a list of bubbles of different
            // heights only estimates how tall it is.
            property bool followLatest: true
            property string keptId: ""
            property real keptOffset: 0
            property string lastThreadId: ""
            function keepPlace() {
                if (followLatest) {
                    positionViewAtEnd()
                    return
                }
                const index = page.messages.findIndex(m => m.id === keptId)
                if (index < 0)
                    return
                positionViewAtIndex(index, ListView.Beginning)
                const bubble = itemAtIndex(index)
                if (bubble)
                    contentY = bubble.y + keptOffset
            }
            function notePlace() {
                // From originY, which moves as bubbles load. Closer than one wheel notch.
                followLatest = originY + contentHeight + bottomMargin - contentY - height <= 8
                // Bubbles have gaps between them: look a little lower if this hits one.
                let index = indexAt(width / 2, contentY)
                if (index < 0)
                    index = indexAt(width / 2, contentY + spacing + 1)
                const bubble = index >= 0 ? itemAtIndex(index) : null
                if (bubble) {
                    keptId = page.messages[index].id
                    keptOffset = contentY - bubble.y
                }
            }
            onModelChanged: {
                if (Messages.thread !== lastThreadId) {
                    lastThreadId = Messages.thread
                    followLatest = true
                }
                forceLayout()
                keepPlace()
            }
            onContentHeightChanged: if (followLatest) Qt.callLater(positionViewAtEnd)
            onHeightChanged: if (followLatest) Qt.callLater(positionViewAtEnd)
            // Before it ends, or the height changing as bubbles load pulls the reader
            // back down mid-scroll.
            onMovementStarted: followLatest = false
            onMovementEnded: notePlace()
            onFlickEnded: notePlace()
            onAtYBeginningChanged: {
                if (atYBeginning && Messages.more && count > 0 && !Messages.loadingOlder) {
                    notePlace()
                    followLatest = false
                    Messages.loadOlder()
                }
            }
            header: Item {
                width: messageList.width
                height: Messages.loadingOlder ? 40 : 0
                Spinner { anchors.centerIn: parent; visible: Messages.loadingOlder }
            }
            delegate: Bubble {
                required property var modelData
                required property int index
                width: messageList.width
                message: modelData
                previous: index > 0 ? page.messages[index - 1] : null
            }
            Txt {
                anchors.centerIn: parent
                visible: page.openThread && page.messages.length === 0 && (page.ready || Messages.currentIsChat)
                muted: true
                text: Messages.currentIsChat ? qsTr("No mirrored messages yet") : qsTr("No messages")
            }
        }

        // The image or voice note about to go as MMS.
        Rectangle {
            id: attachmentBar
            visible: (!!page.openThread || page.composing) && !composer.isChat && Messages.hasAttachment
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: 16
            anchors.rightMargin: 16
            anchors.bottom: composerBar.top
            anchors.bottomMargin: 8
            height: 48
            radius: Theme.rowRadius
            color: Theme.raised
            border.width: Messages.attachmentError.length > 0 ? 1 : 0
            border.color: Theme.tintRed

            RoundedImage {
                id: attThumb
                x: 6
                anchors.verticalCenter: parent.verticalCenter
                width: 36; height: 36
                radius: Theme.radiusSm
                visible: Messages.attachmentPreview.length > 0
                source: Messages.attachmentPreview
                sourceSize: Qt.size(72, 72)
                fillMode: Image.PreserveAspectCrop
            }
            Txt {
                id: attName
                anchors.left: attThumb.visible ? attThumb.right : parent.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, (parent.width - x - removeAttBtn.width - 16) / 2)
                text: Messages.attachmentName
            }
            Txt {
                anchors.left: attName.right
                anchors.leftMargin: 10
                anchors.right: removeAttBtn.left
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                role: "bodySmall"
                color: Messages.attachmentError.length > 0 ? Theme.tintRed : Theme.surfaceContentVariant
                text: Messages.attachmentError.length > 0
                    ? Messages.attachmentError
                    : qsTr("%1 · MMS").arg(Messages.attachmentSize)
            }
            IconButton {
                id: removeAttBtn
                anchors.right: parent.right
                anchors.rightMargin: 6
                anchors.verticalCenter: parent.verticalCenter
                iconPath: Icons.close
                label: qsTr("Remove attachment")
                onClicked: Messages.clearAttachment()
            }
        }

        // Writing a text or a reply in a chat app.
        Item {
            id: composerBar
            visible: !!page.openThread || page.composing
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: composer.height + 24

            Rectangle {
                width: parent.width
                height: 1
                color: Theme.line
            }

            // Long texts: how many SMS they take.
            Txt {
                anchors.right: parent.right
                anchors.rightMargin: 16
                anchors.bottom: parent.top
                anchors.bottomMargin: 4
                visible: !composer.isChat && input.text.length > 120
                role: "bodySmall"
                muted: true
                text: qsTr("%1 characters · %n text(s)", "", page.smsCount(input.text)).arg(input.text.length)
            }

            Rectangle {
                id: composer
                readonly property bool isChat: Messages.currentIsChat && !page.composing
                readonly property bool groupSms: !isChat && !!page.openThread && page.openThread.group
                readonly property bool chatReplyDisabled: isChat && !Messages.currentCanReply
                readonly property bool inputDisabled: groupSms || chatReplyDisabled
                readonly property bool validAttachment: !isChat && Messages.hasAttachment && Messages.attachmentError.length === 0
                readonly property bool hasAttachmentError: !isChat && Messages.hasAttachment && Messages.attachmentError.length > 0
                readonly property bool canSend: !Messages.sending
                    && !inputDisabled
                    && !hasAttachmentError
                    && (isChat ? Messages.currentCanReply : page.ready)
                    && (input.text.trim().length > 0 || validAttachment)
                    && (!page.composing || toField.text.trim().length > 0)
                function focusText() { input.forceActiveFocus() }
                function send() {
                    if (!canSend)
                        return
                    if (page.composing) {
                        // A name typed in To is its contact's number, as Return makes it.
                        if (!/\d/.test(toField.text) && page.recipientMatches.length > 0)
                            toField.text = page.recipientMatches[0].number
                        Messages.sendTo(toField.text.trim(), input.text)
                    }
                    else
                        Messages.send(input.text)
                    input.text = ""
                    delete page.drafts[page.draftKey]
                    page.composing = false
                    toField.text = ""
                }
                anchors.left: parent.left
                anchors.leftMargin: 16
                anchors.right: sendButton.left
                anchors.rightMargin: 8
                y: 10
                height: Math.min(Math.max(input.contentHeight + 16, 34), 140)
                radius: Theme.rowRadius
                color: Theme.raised
                border.width: input.activeFocus ? 1 : 0
                border.color: Theme.focusRing

                IconButton {
                    id: attachButton
                    anchors.left: parent.left
                    anchors.leftMargin: 2
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 2
                    visible: !composer.isChat && !composer.groupSms
                    enabled: page.ready && !Messages.sending
                    iconPath: Icons.photo
                    iconColor: Theme.tintCoral
                    label: qsTr("Attach image or voice note (right-click pastes)")
                    onClicked: imageFileDialog.open()
                    TapHandler {
                        acceptedButtons: Qt.RightButton
                        onTapped: {
                            if (Messages.pasteClipboardImage())
                                composer.focusText()
                        }
                    }
                }

                Flickable {
                    id: inputScroll
                    anchors.left: attachButton.visible ? attachButton.right : parent.left
                    anchors.leftMargin: attachButton.visible ? 2 : 10
                    anchors.right: parent.right
                    anchors.rightMargin: 10
                    anchors.top: parent.top
                    // A single line sits in the middle of the box.
                    anchors.topMargin: Math.max(8, (composer.height - input.contentHeight) / 2)
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: Math.max(8, (composer.height - input.contentHeight) / 2)
                    contentHeight: input.contentHeight
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    TextEdit {
                        id: input
                        width: inputScroll.width
                        wrapMode: TextEdit.Wrap
                        readOnly: composer.inputDisabled
                        font.family: Theme.fontUi
                        font.pixelSize: Theme.typography.body.size
                        color: Theme.surfaceContent
                        selectionColor: Theme.selection
                        selectedTextColor: Theme.surfaceContent
                        Accessible.name: composer.isChat
                            ? qsTr("Reply in %1").arg(Messages.currentAppName)
                            : qsTr("Text message")
                        // Enter sends; Shift+Enter starts a new line; Ctrl+V pastes a clipboard image.
                        Keys.onPressed: (event) => {
                            if (!composer.isChat && !composer.groupSms
                                && ((event.key === Qt.Key_V && (event.modifiers & Qt.ControlModifier))
                                    || event.matches(StandardKey.Paste))) {
                                if (Messages.pasteClipboardImage()) {
                                    event.accepted = true
                                    return
                                }
                            }
                            if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && !(event.modifiers & Qt.ShiftModifier)) {
                                composer.send()
                                event.accepted = true
                            }
                        }
                        onCursorRectangleChanged: {
                            if (cursorRectangle.y < inputScroll.contentY)
                                inputScroll.contentY = cursorRectangle.y
                            else if (cursorRectangle.y + cursorRectangle.height > inputScroll.contentY + inputScroll.height)
                                inputScroll.contentY = cursorRectangle.y + cursorRectangle.height - inputScroll.height
                        }
                        Txt {
                            visible: input.text.length === 0
                            muted: true
                            width: input.width
                            text: composer.chatReplyDisabled
                                ? qsTr("Replies work while the notification is on your phone")
                                : composer.isChat
                                    ? qsTr("Reply in %1").arg(Messages.currentAppName)
                                    : composer.groupSms
                                        ? qsTr("Group texts can't be sent from the PC yet")
                                        : Messages.hasAttachment
                                            ? qsTr("Caption (optional)")
                                            : page.openThread ? qsTr("Text %1").arg(page.openThread.title)
                                            : qsTr("Text message")
                        }
                    }
                }
            }
            Button {
                id: sendButton
                anchors.right: parent.right
                anchors.rightMargin: 16
                anchors.bottom: composer.bottom
                text: qsTr("Send")
                busy: Messages.sending
                enabled: composer.canSend
                onClicked: composer.send()
            }
        }
    }

    // A conversation in the list: name and time, then the latest text.
    component ThreadRow: Item {
        id: row
        property var thread
        readonly property bool selected: thread.id === Messages.thread
        readonly property bool isChat: !!thread.isChat
        // Only the phone's own SMS app can mark texts read, so the one being
        // read here would otherwise keep its unread look.
        readonly property int unread: selected ? 0 : thread.unread
        height: 52
        activeFocusOnTab: true
        Accessible.role: Accessible.ListItem
        Accessible.name: thread.title + (isChat && thread.appName ? " (" + thread.appName + ")" : "") + ". " + thread.snippet
        Accessible.onPressAction: open()
        function open() {
            page.composing = false
            Messages.openThread(thread.id)
        }
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                open()
                event.accepted = true
            }
        }

        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: row.selected ? Theme.selection : rowHover.hovered ? Theme.raised : "transparent"
            border.width: Theme.focusVisible(row) ? 2 : 0
            border.color: Theme.focusRing
        }
        Item {
            id: face
            x: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            width: 28; height: 28
            RoundedImage {
                anchors.fill: parent
                radius: width / 2
                visible: row.thread.photo.length > 0 && status === Image.Ready
                source: row.thread.photo
                sourceSize: Qt.size(56, 56)
                fillMode: Image.PreserveAspectCrop
            }
            Avatar {
                anchors.fill: parent
                visible: row.thread.photo.length === 0
                // A contact's initial; a number (no letter to show) or a group gets an icon.
                readonly property bool named: !/^[\d+(#*]/.test(row.thread.title)
                name: row.thread.group || !named ? "" : row.thread.title
                iconPath: row.thread.group ? Icons.messages : named ? "" : Icons.person
                hue: named && !row.thread.group ? page.hueOf(row.thread.title) : -1
            }
            // The chat app's icon, on a mirrored conversation.
            Rectangle {
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.rightMargin: -3
                anchors.bottomMargin: -3
                width: 14; height: 14
                radius: 7
                visible: row.isChat && !!row.thread.appIcon && row.thread.appIcon.length > 0 && appBadgeImg.status === Image.Ready
                color: Theme.surface
                RoundedImage {
                    id: appBadgeImg
                    anchors.centerIn: parent
                    width: 11; height: 11
                    radius: 5.5
                    source: row.thread.appIcon || ""
                    sourceSize: Qt.size(22, 22)
                    fillMode: Image.PreserveAspectCrop
                }
            }
        }
        Column {
            anchors.left: face.right
            anchors.leftMargin: 10
            anchors.right: parent.right
            anchors.rightMargin: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            Item {
                width: parent.width
                height: name.height
                Txt {
                    id: name
                    anchors.left: parent.left
                    width: Math.min(implicitWidth, parent.width - when.width - (appTag.visible ? appTag.implicitWidth + 16 : 8))
                    text: row.thread.title
                    weight: 500
                }
                Txt {
                    id: appTag
                    anchors.left: name.right
                    anchors.leftMargin: 6
                    anchors.baseline: name.baseline
                    visible: row.isChat && !!row.thread.appName && row.thread.appName.length > 0
                    role: "caption"
                    muted: true
                    text: row.thread.appName || ""
                }
                Txt {
                    id: when
                    anchors.right: parent.right
                    anchors.baseline: name.baseline
                    text: page.shortTime(row.thread.date)
                    role: "caption"
                    color: row.unread > 0 ? Theme.accent : Theme.surfaceContentVariant
                }
            }
            Item {
                width: parent.width
                height: snippet.height
                Txt {
                    id: snippet
                    anchors.left: parent.left
                    anchors.right: badge.visible ? badge.left : parent.right
                    anchors.rightMargin: 8
                    text: {
                        const s = row.thread.snippet || ""
                        const sender = row.thread.lastSender || ""
                        if (row.isChat && row.thread.group && sender.length > 0 && s.indexOf(sender + ":") !== 0)
                            return sender + ": " + s
                        return s
                    }
                    role: "bodySmall"
                    muted: row.unread === 0
                    maximumLineCount: 1
                }
                Rectangle {
                    id: badge
                    anchors.right: parent.right
                    anchors.verticalCenter: snippet.verticalCenter
                    visible: row.unread > 0
                    width: Math.max(16, count.implicitWidth + 8)
                    height: 16
                    radius: 8
                    color: Theme.accent
                    Txt {
                        id: count
                        anchors.centerIn: parent
                        text: row.unread
                        role: "caption"
                        weight: 500
                        color: Theme.accentContent
                    }
                }
            }
        }
        HoverHandler { id: rowHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { onTapped: row.open() }
    }

    // A message: right-aligned when sent from the phone.
    component Bubble: Item {
        id: bubble
        property var message
        property var previous: null
        readonly property bool mine: message.outgoing
        readonly property real maxWidth: Math.min(width * 0.7, 520)
        readonly property bool hasUrl: message.body.indexOf("http://") >= 0 || message.body.indexOf("https://") >= 0
        readonly property bool newDay: {
            if (!previous)
                return true
            // By the calendar alone: 23:55 and 00:02 are two days.
            const d1 = new Date(previous.date)
            const d2 = new Date(message.date)
            return d1.getFullYear() !== d2.getFullYear() || d1.getMonth() !== d2.getMonth() || d1.getDate() !== d2.getDate()
        }
        readonly property bool showSender: !!message.sender && (!previous || previous.sender !== message.sender || newDay)
        height: day.height + sender.height + content.height + meta.height + codeRow.height + 4

        HoverHandler { id: bubbleHover }

        // The day, above its first message.
        Txt {
            id: day
            width: parent.width
            height: bubble.newDay ? implicitHeight + 20 : 0
            visible: bubble.newDay
            topPadding: 8
            horizontalAlignment: Text.AlignHCenter
            role: "caption"
            muted: true
            text: bubble.newDay ? page.dayLabel(bubble.message.date) : ""
        }
        // Who wrote it, in a group.
        Txt {
            id: sender
            anchors.top: day.bottom
            x: 20
            height: bubble.showSender ? implicitHeight + 2 : 0
            visible: bubble.showSender
            role: "bodySmall"
            muted: true
            text: bubble.message.sender || ""
        }

        Rectangle {
            id: content
            anchors.top: sender.bottom
            anchors.right: bubble.mine ? parent.right : undefined
            anchors.left: bubble.mine ? undefined : parent.left
            anchors.leftMargin: 16
            anchors.rightMargin: 16
            readonly property var audioList: bubble.message.audios || []
            width: Math.max(pictures.width, audiosCol.width, Math.min(text.implicitWidth, bubble.maxWidth - 24) + 24)
            height: pictures.height + audiosCol.height + (text.visible ? text.height + 16 : 0)
            radius: 10
            color: bubble.mine ? Theme.wash(Theme.accent) : Theme.raised
            Column {
                id: pictures
                width: bubble.message.images.length > 0 ? Math.min(260, bubble.maxWidth) : 0
                Repeater {
                    model: bubble.message.images
                    delegate: Item {
                        id: picItem
                        required property string modelData
                        width: pictures.width
                        height: modelData.length > 0 && picture.status === Image.Ready
                            ? Math.min(320, width * picture.implicitImageHeight / Math.max(1, picture.implicitImageWidth))
                            : 140
                        activeFocusOnTab: modelData.length > 0
                        Accessible.role: Accessible.Button
                        Accessible.name: qsTr("Open image attachment")
                        Accessible.onPressAction: if (picItem.modelData.length > 0) msgImagePreviewSheet.openFor(picItem.modelData)
                        Keys.onPressed: (event) => {
                            if (picItem.modelData.length > 0 && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
                                msgImagePreviewSheet.openFor(picItem.modelData)
                                event.accepted = true
                            }
                        }
                        RoundedImage {
                            id: picture
                            anchors.fill: parent
                            radius: 10
                            source: parent.modelData
                            fillMode: Image.PreserveAspectCrop
                            sourceSize.width: 520
                        }
                        Rectangle {
                            anchors.fill: parent
                            radius: 10
                            color: "transparent"
                            border.width: 2
                            border.color: Theme.focusRing
                            visible: Theme.focusVisible(picItem)
                        }
                        Spinner { anchors.centerIn: parent; visible: parent.modelData.length === 0 }
                        TapHandler { onTapped: if (parent.modelData.length > 0) msgImagePreviewSheet.openFor(parent.modelData) }
                        HoverHandler { cursorShape: parent.modelData.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor }
                    }
                }
            }
            Column {
                id: audiosCol
                anchors.top: pictures.bottom
                width: content.audioList.length > 0 ? Math.min(240, bubble.maxWidth) : 0
                spacing: 4
                Repeater {
                    model: content.audioList
                    delegate: Rectangle {
                        id: audioPill
                        required property var modelData
                        width: audiosCol.width
                        height: 44
                        radius: 14
                        color: "transparent"
                        Row {
                            x: 12
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 10
                            Rectangle {
                                width: 28; height: 28; radius: 14
                                anchors.verticalCenter: parent.verticalCenter
                                color: Theme.selection
                                Icon {
                                    anchors.centerIn: parent
                                    width: 14; height: 14
                                    path: Icons.speaker
                                    color: Theme.tintPink
                                }
                            }
                            Column {
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 1
                                Txt {
                                    text: qsTr("Voice note")
                                }
                                Txt {
                                    role: "caption"
                                    muted: true
                                    text: (audioPill.modelData.size || "") + (audioPill.modelData.url && audioPill.modelData.url.length > 0 ? qsTr(" · Click to play") : qsTr(" · Loading…"))
                                }
                            }
                        }
                        HoverHandler { cursorShape: audioPill.modelData.url && audioPill.modelData.url.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor }
                        TapHandler {
                            onTapped: {
                                if (audioPill.modelData.url && audioPill.modelData.url.length > 0)
                                    Qt.openUrlExternally(audioPill.modelData.url)
                            }
                        }
                    }
                }
            }
            TextEdit {
                id: text
                anchors.top: audiosCol.bottom
                anchors.topMargin: 8
                x: 12
                width: Math.min(implicitWidth, bubble.maxWidth - 24)
                visible: bubble.message.body.length > 0 || bubble.message.attachments > 0
                text: bubble.message.body.length > 0
                    ? (bubble.hasUrl ? page.linkified(bubble.message.body) : bubble.message.body)
                    : qsTr("Attachment (open it on the phone)")
                readOnly: true
                selectByMouse: true
                wrapMode: TextEdit.Wrap
                textFormat: bubble.hasUrl ? TextEdit.RichText : TextEdit.PlainText
                onLinkActivated: (link) => Qt.openUrlExternally(link)
                HoverHandler { cursorShape: text.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.IBeamCursor }
                font.family: Theme.fontUi
                font.pixelSize: Theme.typography.body.size
                font.italic: bubble.message.body.length === 0
                color: Theme.surfaceContent
                selectionColor: Theme.selection
                selectedTextColor: Theme.surfaceContent
            }
        }
        Txt {
            id: meta
            anchors.top: content.bottom
            anchors.topMargin: 3
            anchors.right: bubble.mine ? content.right : undefined
            anchors.left: bubble.mine ? undefined : content.left
            anchors.leftMargin: 6
            anchors.rightMargin: 6
            role: "caption"
            color: bubble.message.status === "failed" ? Theme.tintRed : Theme.surfaceContentVariant
            text: bubble.message.status === "failed" ? qsTr("Not sent")
                : bubble.message.status === "pending" ? qsTr("Sending…")
                // The day is in the separator above.
                : new Date(bubble.message.date).toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
        }
        // Copy the text (shown while hovered).
        IconButton {
            anchors.verticalCenter: content.verticalCenter
            anchors.left: bubble.mine ? undefined : content.right
            anchors.right: bubble.mine ? content.left : undefined
            anchors.margins: 4
            visible: bubble.message.body.length > 0
            opacity: bubbleHover.hovered || activeFocus ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
            iconPath: Icons.copy
            label: qsTr("Copy text")
            onClicked: Messages.copy(bubble.message.body)
        }
        // A one-time code, one click from the clipboard.
        Item {
            id: codeRow
            anchors.top: meta.bottom
            width: parent.width
            height: bubble.message.code ? codeButton.height + 6 : 0
            visible: !!bubble.message.code
            Button {
                id: codeButton
                x: 16
                y: 4
                variant: "tonal"
                size: "sm"
                iconPath: copied ? Icons.check : Icons.copy
                property bool copied: false
                text: copied ? qsTr("Copied") : qsTr("Copy code %1").arg(bubble.message.code || "")
                onClicked: {
                    copied = true
                    copiedTimer.restart()
                    Messages.copy(bubble.message.code)
                }
                Timer { id: copiedTimer; interval: 2000; onTriggered: codeButton.copied = false }
            }
        }
    }

    // Inline lightbox preview for MMS image attachments.
    Sheet {
        id: msgImagePreviewSheet
        property string imageUrl: ""
        cardWidth: 680
        function openFor(url) {
            imageUrl = url || ""
            if (imageUrl.length > 0)
                open()
        }
        Column {
            width: parent.width
            spacing: 12
            Item {
                width: parent.width
                height: Math.max(msgPreviewTitle.height, closeMsgPreviewBtn.height)
                Txt {
                    id: msgPreviewTitle
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    weight: 500
                    text: qsTr("Image attachment")
                }
                IconButton {
                    id: closeMsgPreviewBtn
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: Icons.close
                    label: qsTr("Close")
                    onClicked: msgImagePreviewSheet.close()
                }
            }
            Rectangle {
                width: parent.width
                height: Math.min(480, Math.max(200, msgPreviewImg.status === Image.Ready
                    ? width * msgPreviewImg.implicitHeight / Math.max(1, msgPreviewImg.implicitWidth)
                    : 320))
                radius: Theme.rowRadius
                color: Theme.raised
                clip: true
                Image {
                    id: msgPreviewImg
                    anchors.fill: parent
                    anchors.margins: 8
                    source: msgImagePreviewSheet.imageUrl
                    fillMode: Image.PreserveAspectFit
                    smooth: true
                    mipmap: true
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 8
                Button {
                    variant: "outline"
                    iconPath: Icons.openExternal
                    text: qsTr("Open externally")
                    onClicked: Qt.openUrlExternally(msgImagePreviewSheet.imageUrl)
                }
                Button {
                    text: qsTr("Done")
                    onClicked: msgImagePreviewSheet.close()
                }
            }
        }
    }

    Rectangle {
        anchors.fill: parent
        anchors.margins: 4
        radius: Theme.rowRadius
        color: "transparent"
        border.width: 2
        border.color: Theme.focusRing
        visible: opacity > 0
        opacity: AppController.highlightTarget === "messages:main" ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }
}
