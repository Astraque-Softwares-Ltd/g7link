// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Calls: recent calls (all or missed) and contacts on the left, the active
// call and a dialer on the right. Audio stays on the phone; calls and texts
// started here go through the phone.
Item {
    id: page
    property bool active: false
    // The phone shown (the one on Home).
    property string deviceId
    property string deviceName
    // "recent", "missed" or "contacts".
    property string tab: "recent"
    // Contact shown above the keypad when picked from the list.
    property var selectedContact: null

    signal textRequested(string number, string name)

    readonly property var callLog: { try { return JSON.parse(PhoneCall.callLog) } catch (e) { return [] } }
    readonly property var contacts: { try { return JSON.parse(PhoneCall.contacts) } catch (e) { return [] } }
    readonly property var sims: { try { return JSON.parse(PhoneCall.sims || "[]") } catch (e) { return [] } }
    readonly property string query: search.text.trim().toLocaleLowerCase()

    readonly property var shownCalls: callLog.filter(c => (tab !== "missed" || c.direction === "missed")
        && (query.length === 0 || ((c.name || "") + " " + c.number).toLocaleLowerCase().indexOf(query) >= 0))

    readonly property var shownContacts: query.length === 0 ? contacts
        : contacts.filter(c => {
            const nums = (c.numbers || []).map(n => n.number + " " + (n.label || "")).join(" ")
            return (c.name + " " + nums).toLocaleLowerCase().indexOf(query) >= 0
        })

    // The number Enter calls: the one in the dialer, or else the latest call.
    readonly property string callBackNumber: dialInput.text.trim().length > 0 ? dialInput.text.trim()
        : (callLog.length > 0 ? callLog[0].number : "")
    readonly property bool canCall: callBackNumber.length > 0 && PhoneCall.canDial && !PhoneCall.dialing
    readonly property bool typing: dialInput.activeFocus || search.input.activeFocus

    // ---- Footer and Actions (Ctrl B) ----
    readonly property string footerLabel: qsTr("Calls")
    readonly property var primaryAction: ({
        label: dialInput.text.trim().length > 0 ? qsTr("Call") : qsTr("Call back"),
        keys: "Enter",
        run: () => page.placeCall(page.callBackNumber)
    })
    readonly property var screenActions: {
        const list = [
            { label: dialInput.text.trim().length > 0 ? qsTr("Call %1").arg(dialInput.text.trim()) : qsTr("Call back"),
              detail: page.callBackNumber, icon: Icons.call, tint: Theme.tintTeal, keys: "Enter",
              enabled: page.canCall, run: () => page.placeCall(page.callBackNumber) },
            { label: qsTr("Text this number"), detail: page.callBackNumber, icon: Icons.messages, tint: Theme.tintBlue,
              enabled: page.callBackNumber.length > 0,
              run: () => page.startText(page.callBackNumber, page.dialMatchedContact ? page.dialMatchedContact.name : "") },
            { label: qsTr("All calls"), icon: Icons.history, tint: Theme.tintTeal, checked: page.tab === "recent",
              run: () => { page.tab = "recent" } },
            { label: qsTr("Missed calls"), icon: Icons.callMissed, tint: Theme.tintRed, checked: page.tab === "missed",
              run: () => { page.tab = "missed" } },
            { label: qsTr("Contacts"), icon: Icons.person, tint: Theme.tintTeal, checked: page.tab === "contacts",
              run: () => { page.tab = "contacts" } },
            { label: qsTr("Search"), icon: Icons.search, keys: "Ctrl F", run: () => search.input.forceActiveFocus() },
            { label: qsTr("Clear number"), icon: Icons.close, enabled: dialInput.text.length > 0,
              run: () => { dialInput.text = "" } },
            { label: qsTr("Refresh"), icon: Icons.refresh,
              enabled: PhoneCall.logStatus !== "loading" && PhoneCall.contactsStatus !== "loading",
              run: () => PhoneCall.refresh() }
        ]
        if (PhoneCall.moreLog)
            list.push({ label: qsTr("Load older calls"), icon: Icons.history, enabled: !PhoneCall.loadingOlder,
                        run: () => PhoneCall.loadOlder() })
        for (const s of page.sims)
            list.push({ label: qsTr("Call with %1").arg(s.label), icon: Icons.signal, checked: PhoneCall.sim === s.id,
                        enabled: PhoneCall.canDial, run: () => PhoneCall.chooseSim(s.id) })
        if (!PhoneCall.hfpConnected && PhoneCall.hfpDetail.length > 0)
            list.push(PhoneCall.hfpAvailable
                ? { label: PhoneCall.hfpConnecting ? qsTr("Connecting…") : qsTr("Connect PC audio"), icon: Icons.speaker,
                    tint: Theme.tintTeal, enabled: !PhoneCall.hfpConnecting, run: () => PhoneCall.connectPcAudio() }
                : { label: qsTr("Open Bluetooth settings"), icon: Icons.bluetooth, tint: Theme.tintBlue,
                    run: () => PhoneCall.openBluetoothSettings() })
        return list
    }

    // Digits for matching a typed number to a contact.
    function digits(s) {
        const d = (s || "").replace(/\D/g, "")
        return d.length > 9 ? d.slice(d.length - 9) : d
    }

    // Contact matching the number currently in the dialer, if any.
    readonly property var dialMatchedContact: {
        const d = digits(dialInput.text)
        if (d.length < 3) return null
        for (let i = 0; i < contacts.length; i++) {
            const c = contacts[i]
            const nums = c.numbers || []
            for (let j = 0; j < nums.length; j++) {
                if (digits(nums[j].number) === d)
                    return { name: c.name, label: nums[j].label || "" }
            }
        }
        return null
    }

    // A stable color for a contact.
    function hueOf(name) {
        let h = 0
        for (let i = 0; i < name.length; i++)
            h = (h * 31 + name.charCodeAt(i)) % 360
        return h / 360
    }

    // "Today", "Yesterday", or the date, for call log separators.
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

    // "1:05" or "42s" for a completed call's duration.
    function formatDuration(seconds) {
        if (!seconds || seconds <= 0) return ""
        const h = Math.floor(seconds / 3600)
        const m = Math.floor((seconds % 3600) / 60)
        const s = seconds % 60
        if (h > 0)
            return h + ":" + String(m).padStart(2, "0") + ":" + String(s).padStart(2, "0")
        if (m > 0)
            return m + ":" + String(s).padStart(2, "0")
        return qsTr("%1s").arg(s)
    }

    function placeCall(number) {
        const n = (number || "").trim()
        if (n.length === 0 || !PhoneCall.canDial || PhoneCall.dialing)
            return
        PhoneCall.dial(n)
    }

    function startText(number, name) {
        const n = (number || "").trim()
        if (n.length === 0)
            return
        page.textRequested(n, name || "")
    }

    function consumePendingDial() {
        if (AppController.pendingDial.length > 0) {
            dialInput.text = AppController.pendingDial
            AppController.pendingDial = ""
        }
    }

    function load() {
        if (active && deviceId.length > 0)
            PhoneCall.open(deviceId)
        consumePendingDial()
    }

    Connections {
        target: AppController
        function onPendingDialChanged() { page.consumePendingDial() }
    }

    onActiveChanged: if (active) load()
    Component.onCompleted: load()
    onDeviceIdChanged: {
        selectedContact = null
        // Another phone's contacts: the last search doesn't apply. A typed
        // number stays, to call it from this phone.
        search.text = ""
        load()
    }

    visible: active

    Shortcut {
        sequences: [StandardKey.Find]
        enabled: page.active && page.deviceId.length > 0
        onActivated: search.input.forceActiveFocus()
    }
    Shortcut {
        sequence: "Esc"
        enabled: page.active && (page.selectedContact !== null || search.text.length > 0)
        onActivated: {
            if (search.text.length > 0)
                search.text = ""
            else
                page.selectedContact = null
        }
    }
    Shortcut {
        sequences: ["Return", "Enter"]
        enabled: page.active && !page.typing && page.canCall
        onActivated: page.placeCall(page.callBackNumber)
    }

    readonly property bool phoneOffline: page.deviceId.length === 0
        || (PhoneCall.logStatus === "offline" && PhoneCall.contactsStatus === "offline"
            && page.callLog.length === 0 && page.contacts.length === 0)

    // ---- No phone paired, or phone offline with nothing cached ----
    Column {
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 380)
        spacing: 8
        visible: page.phoneOffline
        Icon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 28; height: 28
            path: Icons.call
            color: Theme.tintTeal
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            weight: 500
            text: page.deviceId.length === 0 ? qsTr("No phone paired")
                : qsTr("%1 isn't connected").arg(page.deviceName)
        }
    }

    // ---- Recent calls and contacts ----
    Item {
        id: list
        visible: !page.phoneOffline
        anchors.left: parent.left
        anchors.right: aside.left
        height: parent.height

        Item {
            id: listHeader
            x: 8
            y: 8
            width: parent.width - 16
            height: 36
            Row {
                id: tabs
                x: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    rightPadding: 6
                    text: qsTr("Recent")
                    role: "label"
                    muted: true
                }
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("All")
                    selected: page.tab === "recent"
                    onClicked: page.tab = "recent"
                }
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Missed")
                    selected: page.tab === "missed"
                    onClicked: page.tab = "missed"
                }
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Contacts")
                    selected: page.tab === "contacts"
                    onClicked: page.tab = "contacts"
                }
            }
            // Search: by name or number.
            Field {
                id: search
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: Math.max(120, Math.min(240, parent.width - tabs.width - 24))
                height: 28
                iconPath: Icons.search
                placeholder: page.tab === "contacts" ? qsTr("Search contacts") : qsTr("Search calls")
                Keys.onEscapePressed: (event) => {
                    event.accepted = search.text.length > 0
                    search.text = ""
                }
            }
        }

        // ---- Recent and missed calls ----
        Item {
            anchors.top: listHeader.bottom
            anchors.topMargin: 4
            anchors.bottom: parent.bottom
            x: 8
            width: parent.width - 16
            visible: page.tab !== "contacts"

            Spinner {
                anchors.centerIn: parent
                visible: PhoneCall.logStatus === "loading" && page.callLog.length === 0
            }

            // Call log off, not allowed, or failed.
            Column {
                anchors.centerIn: parent
                width: parent.width - 40
                spacing: 8
                visible: page.callLog.length === 0 && PhoneCall.logStatus !== "loading" && PhoneCall.logStatus !== "ready"
                Txt {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    weight: 500
                    text: PhoneCall.logStatus === "off" ? qsTr("Calls are off for %1").arg(page.deviceName)
                        : PhoneCall.logStatus === "unsupported" ? qsTr("Allow call log on %1").arg(page.deviceName)
                        : PhoneCall.logStatus === "offline" ? qsTr("%1 isn't connected").arg(page.deviceName)
                        : qsTr("Couldn't load recent calls")
                }
                Txt {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    visible: PhoneCall.logStatus === "off"
                    role: "bodySmall"
                    muted: true
                    text: qsTr("Turn on Calls in Settings")
                }
                Button {
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: PhoneCall.logStatus === "failed"
                    variant: "tonal"
                    size: "sm"
                    text: qsTr("Try again")
                    onClicked: PhoneCall.refresh()
                }
            }

            ListView {
                id: recentList
                anchors.fill: parent
                visible: page.callLog.length > 0 || PhoneCall.logStatus === "ready"
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                model: page.shownCalls
                onAtYEndChanged: if (atYEnd && PhoneCall.moreLog && count > 0) PhoneCall.loadOlder()
                delegate: CallRow {
                    required property var modelData
                    required property int index
                    width: recentList.width
                    entry: modelData
                    previous: index > 0 ? page.shownCalls[index - 1] : null
                }
                footer: Item {
                    width: recentList.width
                    height: PhoneCall.loadingOlder ? 40 : 0
                    Spinner { anchors.centerIn: parent; visible: PhoneCall.loadingOlder }
                }
                Txt {
                    anchors.centerIn: parent
                    visible: PhoneCall.logStatus === "ready" && page.shownCalls.length === 0
                    muted: true
                    text: page.query.length > 0 ? qsTr("No matches")
                        : page.tab === "missed" ? qsTr("No missed calls") : qsTr("No recent calls")
                }
            }
        }

        // ---- Contacts ----
        Item {
            anchors.top: listHeader.bottom
            anchors.topMargin: 4
            anchors.bottom: parent.bottom
            x: 8
            width: parent.width - 16
            visible: page.tab === "contacts"

            Spinner {
                anchors.centerIn: parent
                visible: PhoneCall.contactsStatus === "loading" && page.contacts.length === 0
            }

            Column {
                anchors.centerIn: parent
                width: parent.width - 40
                spacing: 8
                visible: page.contacts.length === 0 && PhoneCall.contactsStatus !== "loading" && PhoneCall.contactsStatus !== "ready"
                Txt {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    weight: 500
                    text: PhoneCall.contactsStatus === "off" ? qsTr("Contacts are off for %1").arg(page.deviceName)
                        : PhoneCall.contactsStatus === "unsupported" ? qsTr("Allow contacts on %1").arg(page.deviceName)
                        : PhoneCall.contactsStatus === "offline" ? qsTr("%1 isn't connected").arg(page.deviceName)
                        : qsTr("Couldn't load contacts")
                }
                Txt {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    visible: PhoneCall.contactsStatus === "off"
                    role: "bodySmall"
                    muted: true
                    text: qsTr("Turn on Contacts in Settings")
                }
                Button {
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: PhoneCall.contactsStatus === "failed"
                    variant: "tonal"
                    size: "sm"
                    text: qsTr("Try again")
                    onClicked: PhoneCall.refresh()
                }
            }

            ListView {
                id: contactList
                anchors.fill: parent
                visible: page.contacts.length > 0 || PhoneCall.contactsStatus === "ready"
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                model: page.shownContacts
                delegate: ContactRow {
                    required property var modelData
                    required property int index
                    width: contactList.width
                    contact: modelData
                    previous: index > 0 ? page.shownContacts[index - 1] : null
                }
                Txt {
                    anchors.centerIn: parent
                    visible: PhoneCall.contactsStatus === "ready" && page.shownContacts.length === 0
                    muted: true
                    text: page.query.length > 0 ? qsTr("No matches") : qsTr("No contacts")
                }
            }
        }
    }

    // ---- Right column: the active call, a picked contact and the dialer ----
    Flickable {
        id: aside
        visible: !page.phoneOffline
        anchors.right: parent.right
        width: page.phoneOffline ? 0 : Math.min(Theme.columnWidth + 20, Math.max(260, page.width * 0.4))
        height: parent.height
        contentHeight: asideColumn.height + 40
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Rectangle {
            width: 1
            height: Math.max(aside.height, aside.contentHeight)
            color: Theme.line
        }

        Column {
            id: asideColumn
            x: 20
            y: 20
            width: aside.width - 40
            spacing: 14

            // The call in progress on the phone.
            CallCard {
                width: parent.width
                visible: PhoneCall.active && PhoneCall.device === page.deviceId
            }

            // A picked contact: each number, to copy, text or call.
            Column {
                width: parent.width
                visible: page.selectedContact !== null
                spacing: 2
                Item {
                    width: parent.width
                    height: 36
                    Item {
                        id: detailFace
                        width: 28; height: 28
                        anchors.verticalCenter: parent.verticalCenter
                        RoundedImage {
                            anchors.fill: parent
                            radius: width / 2
                            visible: (page.selectedContact ? page.selectedContact.photo || "" : "").length > 0 && status === Image.Ready
                            source: page.selectedContact ? page.selectedContact.photo || "" : ""
                            sourceSize: Qt.size(56, 56)
                            fillMode: Image.PreserveAspectCrop
                        }
                        Avatar {
                            anchors.fill: parent
                            visible: !page.selectedContact || (page.selectedContact.photo || "").length === 0
                            name: page.selectedContact ? page.selectedContact.name : ""
                            hue: page.selectedContact ? page.hueOf(page.selectedContact.name) : -1
                        }
                    }
                    Txt {
                        anchors.left: detailFace.right
                        anchors.leftMargin: 10
                        anchors.right: detailStar.visible ? detailStar.left : closeDetail.left
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        text: page.selectedContact ? page.selectedContact.name : ""
                        weight: 500
                    }
                    Icon {
                        id: detailStar
                        anchors.right: closeDetail.left
                        anchors.rightMargin: 4
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !!(page.selectedContact && page.selectedContact.starred)
                        width: 14; height: 14
                        path: Icons.star
                        color: Theme.tintAmber
                    }
                    IconButton {
                        id: closeDetail
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        iconPath: Icons.close
                        label: qsTr("Close contact")
                        onClicked: page.selectedContact = null
                    }
                }
                Repeater {
                    model: page.selectedContact ? (page.selectedContact.numbers || []) : []
                    delegate: ActionRow {
                        id: numRow
                        required property var modelData
                        width: parent.width
                        interactive: false
                        title: modelData.number
                        detail: modelData.label || ""
                        IconButton {
                            iconPath: Icons.copy
                            label: qsTr("Copy number")
                            doneLabel: qsTr("Copied")
                            onClicked: {
                                PhoneCall.copy(numRow.modelData.number)
                                flashDone()
                            }
                        }
                        IconButton {
                            iconPath: Icons.messages
                            iconColor: Theme.tintBlue
                            label: qsTr("Text %1").arg(numRow.modelData.number)
                            onClicked: page.startText(numRow.modelData.number, page.selectedContact ? page.selectedContact.name : "")
                        }
                        IconButton {
                            iconPath: Icons.call
                            iconColor: Theme.tintTeal
                            enabled: PhoneCall.canDial && !PhoneCall.dialing
                            label: qsTr("Call %1").arg(numRow.modelData.number)
                            onClicked: {
                                dialInput.text = numRow.modelData.number
                                page.placeCall(numRow.modelData.number)
                            }
                        }
                    }
                }
            }

            // Number or name to call.
            Rectangle {
                width: parent.width
                height: dialMatch.visible ? 56 : 42
                radius: Theme.rowRadius
                color: Theme.raised
                border.width: dialInput.activeFocus ? 1 : 0
                border.color: Theme.focusRing

                Column {
                    anchors.left: parent.left
                    anchors.leftMargin: 36
                    anchors.right: parent.right
                    anchors.rightMargin: 36
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2

                    TextInput {
                        id: dialInput
                        width: parent.width
                        horizontalAlignment: TextInput.AlignHCenter
                        font.family: Theme.fontUi
                        font.pixelSize: 16
                        color: Theme.surfaceContent
                        selectionColor: Theme.selection
                        selectedTextColor: Theme.surfaceContent
                        inputMethodHints: Qt.ImhDialableCharactersOnly
                        clip: true
                        Accessible.name: qsTr("Phone number to call")
                        Keys.onReturnPressed: page.placeCall(dialInput.text)
                        Keys.onEnterPressed: page.placeCall(dialInput.text)
                        Txt {
                            anchors.centerIn: parent
                            visible: dialInput.text.length === 0
                            muted: true
                            size: 16
                            text: qsTr("Number")
                        }
                    }
                    Txt {
                        id: dialMatch
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        visible: page.dialMatchedContact !== null
                        role: "bodySmall"
                        color: Theme.tintTeal
                        text: page.dialMatchedContact
                            ? (page.dialMatchedContact.label.length > 0
                                ? page.dialMatchedContact.name + " · " + page.dialMatchedContact.label
                                : page.dialMatchedContact.name)
                            : ""
                    }
                }
                IconButton {
                    anchors.right: parent.right
                    anchors.rightMargin: 4
                    anchors.verticalCenter: parent.verticalCenter
                    visible: dialInput.text.length > 0
                    iconPath: Icons.backspace
                    label: qsTr("Backspace")
                    onClicked: dialInput.text = dialInput.text.slice(0, -1)
                }
            }

            // 3×4 keypad. In a call a key is a tone (a PIN, say), not a digit
            // left in the number box.
            Grid {
                width: parent.width
                columns: 3
                columnSpacing: 6
                rowSpacing: 6
                Repeater {
                    model: [
                        { digit: "1", sub: "" }, { digit: "2", sub: "ABC" }, { digit: "3", sub: "DEF" },
                        { digit: "4", sub: "GHI" }, { digit: "5", sub: "JKL" }, { digit: "6", sub: "MNO" },
                        { digit: "7", sub: "PQRS" }, { digit: "8", sub: "TUV" }, { digit: "9", sub: "WXYZ" },
                        { digit: "*", sub: "" }, { digit: "0", sub: "+" }, { digit: "#", sub: "" }
                    ]
                    delegate: Rectangle {
                        id: padKey
                        required property var modelData
                        width: (asideColumn.width - 12) / 3
                        height: 46
                        radius: Theme.rowRadius
                        activeFocusOnTab: true
                        color: padTap.pressed ? Theme.selection : (padHover.hovered ? Theme.selection : Theme.raised)
                        border.width: Theme.focusVisible(padKey) ? 2 : 0
                        border.color: Theme.focusRing

                        function tapDigit(ch) {
                            if (PhoneCall.active && PhoneCall.controls)
                                PhoneCall.press(ch)
                            else
                                dialInput.text = dialInput.text + ch
                        }

                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                tapDigit(modelData.digit)
                                event.accepted = true
                            }
                        }

                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.sub.length > 0 ? modelData.digit + " " + modelData.sub : modelData.digit
                        Accessible.onPressAction: tapDigit(modelData.digit)

                        Column {
                            anchors.centerIn: parent
                            Txt {
                                anchors.horizontalCenter: parent.horizontalCenter
                                size: 17
                                text: padKey.modelData.digit
                            }
                            Txt {
                                anchors.horizontalCenter: parent.horizontalCenter
                                height: 13
                                role: "caption"
                                size: 10
                                muted: true
                                text: padKey.modelData.sub
                            }
                        }
                        HoverHandler { id: padHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler {
                            id: padTap
                            onTapped: padKey.tapDigit(padKey.modelData.digit)
                            onLongPressed: {
                                if (padKey.modelData.digit === "0")
                                    padKey.tapDigit("+")
                            }
                        }
                    }
                }
            }

            // Call: teal, the color of calls.
            Rectangle {
                id: callButton
                readonly property bool usable: dialInput.text.trim().length > 0 && PhoneCall.canDial && !PhoneCall.dialing
                width: parent.width
                height: 40
                radius: Theme.rowRadius
                color: Theme.tintTeal
                opacity: usable ? 1 : 0.38
                activeFocusOnTab: usable
                Accessible.role: Accessible.Button
                Accessible.name: callLabel.text
                Accessible.onPressAction: if (usable) page.placeCall(dialInput.text)
                Keys.onPressed: (event) => {
                    if (callButton.usable && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
                        page.placeCall(dialInput.text)
                        event.accepted = true
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    radius: parent.radius
                    color: Theme.surface
                    opacity: callMouse.pressed ? 0.16 : (callMouse.containsMouse && callButton.usable ? 0.08 : 0)
                }
                Row {
                    anchors.centerIn: parent
                    spacing: 8
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16; height: 16
                        path: Icons.call
                        color: Theme.surface
                    }
                    Txt {
                        id: callLabel
                        anchors.verticalCenter: parent.verticalCenter
                        text: PhoneCall.dialing ? qsTr("Calling…") : qsTr("Call")
                        weight: 500
                        color: Theme.surface
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: -3
                    radius: parent.radius + 3
                    color: "transparent"
                    border.width: 2
                    border.color: Theme.focusRing
                    visible: Theme.focusVisible(callButton)
                }
                MouseArea {
                    id: callMouse
                    anchors.fill: parent
                    enabled: callButton.usable
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: page.placeCall(dialInput.text)
                }
            }

            Row {
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: 6
                TextAction {
                    iconPath: Icons.messages
                    iconColor: Theme.tintBlue
                    text: qsTr("Text")
                    enabled: dialInput.text.trim().length > 0
                    onClicked: page.startText(dialInput.text, page.dialMatchedContact ? page.dialMatchedContact.name : "")
                }
                TextAction {
                    visible: dialInput.text.length > 0
                    iconPath: Icons.close
                    iconColor: Theme.surfaceContentVariant
                    text: qsTr("Clear")
                    onClicked: dialInput.text = ""
                }
            }

            Txt {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                role: "bodySmall"
                muted: true
                text: qsTr("Rings on your phone")
            }

            // Which SIM calls go out on, when the phone has more than one
            // (otherwise Android asks on the phone, where nobody sees it).
            Column {
                width: parent.width
                visible: page.sims.length > 1 && PhoneCall.canDial
                spacing: 4
                SectionLabel {
                    width: parent.width
                    text: qsTr("Call with")
                }
                Segmented {
                    options: page.sims.map(s => ({ value: s.id, label: s.label }))
                    value: PhoneCall.sim
                    onPicked: (v) => PhoneCall.chooseSim(v)
                }
            }

            LockChip {
                anchors.horizontalCenter: parent.horizontalCenter
                readonly property var dialFeature: AppController.capsRevision >= 0 && page.deviceId.length > 0
                    ? AppController.featureState(page.deviceId, "calls.dial") : ({})
                visible: !PhoneCall.canDial && label.length > 0
                feature: dialFeature
            }

            // Bluetooth call audio on this PC.
            Column {
                width: parent.width
                visible: !PhoneCall.active && PhoneCall.hfpDetail.length > 0
                spacing: 2
                ActionRow {
                    width: parent.width
                    interactive: false
                    iconPath: Icons.speaker
                    tint: PhoneCall.hfpConnected ? Theme.tintTeal : Theme.surfaceContentVariant
                    title: PhoneCall.hfpConnected ? qsTr("PC call audio connected")
                        : PhoneCall.hfpAvailable ? qsTr("PC call audio ready")
                        : qsTr("PC call audio (Bluetooth)")
                }
                Txt {
                    x: Theme.rowPadding
                    width: parent.width - 2 * Theme.rowPadding
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                    text: PhoneCall.hfpDetail
                }
                TextAction {
                    x: Theme.rowPadding - 8
                    visible: !PhoneCall.hfpConnected
                    enabled: !PhoneCall.hfpConnecting
                    text: PhoneCall.hfpConnecting ? qsTr("Connecting…")
                        : PhoneCall.hfpAvailable ? qsTr("Connect PC audio") : qsTr("Open Bluetooth settings")
                    onClicked: {
                        if (PhoneCall.hfpAvailable)
                            PhoneCall.connectPcAudio()
                        else
                            PhoneCall.openBluetoothSettings()
                    }
                }
            }
        }
    }

    // ---- A call in the log ----
    component CallRow: Column {
        id: row
        property var entry
        property var previous: null
        readonly property bool newDay: !previous || new Date(previous.date).toDateString() !== new Date(entry.date).toDateString()
        readonly property bool missed: entry.direction === "missed"
        readonly property string displayName: (entry.name && entry.name.length > 0) ? entry.name : entry.number
        readonly property string durText: page.formatDuration(entry.duration)
        readonly property string dirLabel: entry.direction === "missed" ? qsTr("Missed")
            : entry.direction === "outgoing" ? qsTr("Outgoing")
            : entry.direction === "rejected" ? qsTr("Declined")
            : qsTr("Incoming")

        SectionLabel {
            width: row.width
            visible: row.newDay
            text: row.newDay ? page.dayLabel(row.entry.date) : ""
        }
        ActionRow {
            id: callRow
            width: row.width
            iconPath: row.missed ? Icons.callMissed
                : row.entry.direction === "outgoing" ? Icons.callOutgoing : Icons.callIncoming
            tint: row.missed || row.entry.direction === "rejected" ? Theme.tintRed
                : row.entry.direction === "outgoing" ? Theme.tintBlue : Theme.tintTeal
            title: row.displayName
            strong: true
            titleColor: row.missed ? Theme.tintRed : Theme.surfaceContent
            detail: row.durText.length > 0 ? row.dirLabel + " · " + row.durText : row.dirLabel
            meta: page.shortTime(row.entry.date)
            selected: dialInput.text.trim().length > 0 && dialInput.text.trim() === row.entry.number
            accessibleName: row.displayName + ", " + row.dirLabel + ", " + page.shortTime(row.entry.date)
            onClicked: dialInput.text = row.entry.number
            IconButton {
                visible: callRow.hovered || callRow.selected
                iconPath: Icons.messages
                iconColor: Theme.tintBlue
                label: qsTr("Text %1").arg(row.displayName)
                onClicked: page.startText(row.entry.number, row.entry.name || "")
            }
            IconButton {
                enabled: PhoneCall.canDial && !PhoneCall.dialing && row.entry.number.length > 0
                iconPath: Icons.call
                iconColor: Theme.tintTeal
                label: qsTr("Call %1").arg(row.displayName)
                onClicked: {
                    dialInput.text = row.entry.number
                    page.placeCall(row.entry.number)
                }
            }
        }
    }

    // ---- A contact ----
    component ContactRow: Column {
        id: crow
        property var contact
        property var previous: null
        readonly property var primaryEntry: (contact.numbers && contact.numbers.length > 0) ? contact.numbers[0] : null
        readonly property string primaryNumber: primaryEntry ? primaryEntry.number : ""
        readonly property string primaryLabel: primaryEntry && primaryEntry.label ? primaryEntry.label : ""
        // "Favorites" / "Contacts" when not filtering.
        readonly property bool showSection: page.query.length === 0
            && (!previous || previous.starred !== contact.starred)

        function pickContact() {
            page.selectedContact = contact
            if (primaryNumber.length > 0)
                dialInput.text = primaryNumber
        }

        SectionLabel {
            width: crow.width
            visible: crow.showSection
            text: crow.contact.starred ? qsTr("Favorites") : qsTr("Contacts")
        }
        ActionRow {
            id: contactRow
            width: crow.width
            title: crow.contact.name
            strong: true
            detail: {
                const parts = []
                if (crow.primaryLabel.length > 0)
                    parts.push(crow.primaryLabel)
                if (crow.primaryNumber.length > 0)
                    parts.push(crow.primaryNumber)
                const extra = (crow.contact.numbers ? crow.contact.numbers.length : 0) - 1
                if (extra > 0)
                    parts.push(qsTr("+%1 more").arg(extra))
                return parts.join(" · ")
            }
            selected: page.selectedContact !== null && page.selectedContact.id === crow.contact.id
            accessibleName: crow.contact.name + (crow.primaryNumber.length > 0 ? ", " + crow.primaryNumber : "")
            onClicked: crow.pickContact()
            leading: Item {
                width: 24; height: 24
                RoundedImage {
                    anchors.fill: parent
                    radius: width / 2
                    visible: (crow.contact.photo || "").length > 0 && status === Image.Ready
                    source: crow.contact.photo || ""
                    sourceSize: Qt.size(48, 48)
                    fillMode: Image.PreserveAspectCrop
                }
                Avatar {
                    anchors.fill: parent
                    visible: (crow.contact.photo || "").length === 0
                    name: crow.contact.name
                    hue: page.hueOf(crow.contact.name)
                }
            }
            Icon {
                visible: !!crow.contact.starred
                width: 14; height: 14
                path: Icons.star
                color: Theme.tintAmber
            }
            IconButton {
                visible: crow.primaryNumber.length > 0 && (contactRow.hovered || contactRow.selected)
                iconPath: Icons.messages
                iconColor: Theme.tintBlue
                label: qsTr("Text %1").arg(crow.contact.name)
                onClicked: page.startText(crow.primaryNumber, crow.contact.name)
            }
            IconButton {
                visible: crow.primaryNumber.length > 0
                enabled: PhoneCall.canDial && !PhoneCall.dialing
                iconPath: Icons.call
                iconColor: Theme.tintTeal
                label: qsTr("Call %1").arg(crow.contact.name)
                onClicked: {
                    dialInput.text = crow.primaryNumber
                    page.placeCall(crow.primaryNumber)
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
        opacity: AppController.highlightTarget === "calls:main" ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Theme.fadeFast } }
    }
}
