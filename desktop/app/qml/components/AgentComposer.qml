// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The reply box under a thread: one container with the queued messages on
// top, the images to send, the text, and a quiet footer (add image, the
// model and effort, the mode when it isn't the default, Stop, and one round
// primary button). The folder sits under it.
//
// Enter sends; Shift+Enter starts a new line. While the agent works, Enter
// does your default with a new message (steer it, or wait its turn) and
// Alt+Enter the other. `@` lists the folder's files and `/` its commands;
// choosing one inserts it and sends nothing. While a request waits and the
// host takes words, what you type denies it and tells the agent what to do
// instead ("Deny and send"); Ctrl+Enter allows it only while the box is
// empty. While a question waits there is no Send: the box is the
// question's "Something else" and nothing more, and the image button and
// model control rest.
Item {
    id: composer
    // The request the dock shows (`Agents.ask`, parsed; `{}` for none).
    property var ask: ({})
    // The question the dock shows (`Agents.question`, parsed; `{}` for none).
    property var question: ({})
    // `Agents.composer` (`controls`), `Agents.popup` and `Agents.composerSet`, parsed.
    property var controls: ({})
    property var popup: ({})
    property var setText: ({})
    // The host takes words with a denial.
    property bool canSayWhy: false
    // A turn runs or waits.
    property bool running: false
    property bool busy: false
    property string agentName
    // "claude" or "codex".
    property string agentId
    // The folder it runs in, shown under the box.
    property string folder

    // Quote chips of diff lines (§SC9.1): [{ path, first, last }].
    property var quotes: []
    signal send(string text, bool other)
    signal denySend(string token, string text)
    signal allow
    signal stop
    signal attachRequested
    // The model control was clicked: open the picker over (x, y), in the window.
    signal pickerRequested(real x, real y)

    readonly property bool asking: (ask.token || "").length > 0
    readonly property bool questionShown: (question.token || "").length > 0
    readonly property var answer: question.ask || ({})
    readonly property bool answerWords: questionShown && answer.other === true && question.held !== true
    readonly property bool control: controls.control === true
    readonly property var picker: controls.picker || ({})
    readonly property string mode: picker.mode || "supervised"
    readonly property bool canSteer: controls.canSteer === true
    readonly property bool steerFirst: (controls.followUp || "steer") === "steer"
    // The request the words were typed under ("" for none).
    property string typedFor: ""
    // The page sets the text (not the user).
    property bool settingText: false
    readonly property alias text: input.text
    readonly property alias inputFocused: input.activeFocus
    readonly property bool hasText: input.text.trim().length > 0
    // Words now deny the shown request and tell the agent why.
    readonly property bool sayWhy: asking && canSayWhy
    // A message sent now reaches a running turn (steered or queued).
    readonly property bool followUp: running && control
    readonly property bool canSend: !questionShown && (hasText || quotes.length > 0) && !busy && (!running || control)
    readonly property bool canDenySend: sayWhy && hasText && typedFor === (ask.token || "")
        && ask.held !== true && !busy
    readonly property bool canAllow: asking && ask.held !== true && !busy && !hasText
    readonly property bool editable: questionShown ? answerWords : (!running || sayWhy || control)
    // What Enter does with these words while the agent works.
    readonly property bool enterSteers: steerFirst && canSteer
    readonly property bool popupOpen: (popup.kind || "").length > 0
    // The model control and the image button rest while a question waits
    // or a turn runs: settings apply between turns, an answer takes no image.
    readonly property bool settingsOpen: control && !running && !questionShown
    // Tabler icons (MIT, as Icons.qml credits): arrow-up, player-track-next, stack-2.
    readonly property string sendIcon: "M12 5v14M18 11l-6-6M6 11l6-6"
    readonly property string steerIcon: "M3 5v14l8-7zM14 5v14l8-7z"
    readonly property string queueIcon: "M12 4l-8 4 8 4 8-4-8-4M4 12l8 4 8-4M4 16l8 4 8-4"

    function clear() {
        settingText = true
        input.text = ""
        settingText = false
    }
    function submit(other) {
        if (sayWhy && hasText) {
            if (canDenySend) {
                composer.denySend(ask.token, input.text)
                clear()
            }
            return
        }
        if (canSend) {
            composer.send(input.text, other)
            clear()
        }
    }
    function focusInput() {
        input.forceActiveFocus()
    }
    function put(text, cursor) {
        settingText = true
        input.text = text
        input.cursorPosition = Math.max(0, Math.min(cursor, input.length))
        settingText = false
    }
    // A row of the `@` or `/` list (-1: the selected one).
    function choose(index) {
        const json = Agents.popupChoose(index)
        if (json.length === 0)
            return
        let chosen = {}
        try {
            chosen = JSON.parse(json)
        } catch (e) {
            return
        }
        put(chosen.text, chosen.cursor)
        if (chosen.action === "model" || chosen.action === "effort")
            openPicker()
        else if (chosen.action === "plan")
            Agents.pick(false, "mode", "plan")
    }
    function openPicker() {
        if (!settingsOpen)
            return
        const p = modelControl.mapToItem(null, 0, 0)
        composer.pickerRequested(p.x, p.y)
    }

    // The composer's text and cursor reach the bridge once a frame, after
    // the cursor has followed the text.
    property bool pendingTyped: false
    function report(typed) {
        pendingTyped = pendingTyped || typed
        Qt.callLater(flush)
    }
    function flush() {
        Agents.composerEdited(input.text, input.cursorPosition, pendingTyped)
        pendingTyped = false
    }

    // Text the page must take: a draft, an ask's own words, a chosen row.
    property int seenGen: -1
    property int seenN: -1
    onSetTextChanged: {
        const gen = setText.gen === undefined ? -1 : setText.gen
        const n = setText.n || 0
        if (gen === seenGen && n === seenN)
            return
        const newView = gen !== seenGen
        seenGen = gen
        seenN = n
        if (n > 0)
            put(setText.text || "", setText.cursor || 0)
        else if (newView)
            clear()
    }

    implicitHeight: offer.height + box.height + caption.height + 4

    // ---- A newer draft from another device ----
    Item {
        id: offer
        width: parent.width
        height: (composer.controls.offer || "").length > 0 ? 32 : 0
        visible: height > 0
        Row {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: Icons.edit
                color: Theme.surfaceContentVariant
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("A newer draft from %1").arg(composer.controls.offer || "")
                role: "bodySmall"
                muted: true
            }
        }
        Button {
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            variant: "tonal"
            size: "sm"
            text: qsTr("Use it")
            onClicked: Agents.useDraft()
        }
    }

    // ---- The `@` and `/` lists, over the transcript ----
    AgentMentionList {
        visible: composer.popup.kind === "files"
        anchors.bottom: box.top
        anchors.bottomMargin: 6
        width: box.width
        height: visible ? implicitHeight : 0
        popup: composer.popup
        z: 20
        onChosen: (index) => composer.choose(index)
    }
    AgentCommandList {
        visible: composer.popup.kind === "commands"
        anchors.bottom: box.top
        anchors.bottomMargin: 6
        width: box.width
        height: visible ? implicitHeight : 0
        popup: composer.popup
        z: 20
        onChosen: (index) => composer.choose(index)
    }

    Rectangle {
        id: box
        y: offer.height
        width: parent.width
        height: editing.height + queue.height + strip.height + quoteRow.height + inputArea.height + actions.height + 12
        radius: Theme.rowRadius
        color: Theme.raised
        border.width: input.activeFocus ? 1 : 0
        border.color: Theme.focusRing

        // Edit from here: the message is back in the box; Cancel undoes the rewind.
        Rectangle {
            id: editing
            x: 4
            y: 4
            width: parent.width - 8
            height: composer.controls.editing === true ? 28 : 0
            visible: height > 0
            radius: Theme.radiusSm
            color: Theme.selection
            Row {
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                    path: Icons.edit
                    color: Theme.accent
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Editing your message")
                    role: "bodySmall"
                    weight: 500
                }
            }
            Button {
                anchors.right: parent.right
                anchors.rightMargin: 2
                anchors.verticalCenter: parent.verticalCenter
                variant: "text"
                size: "sm"
                text: qsTr("Cancel")
                enabled: !composer.busy
                onClicked: Agents.undoRewind()
            }
        }

        AgentQueue {
            id: queue
            x: 4
            y: 4 + editing.height
            width: parent.width - 8
            height: implicitHeight
            waiting: composer.controls.queue || ({})
            agentName: composer.agentName
            busy: composer.busy
            onSendNow: (id) => Agents.queue("send_now", id, "")
            onRemove: (id) => Agents.queue("remove", id, "")
            onEdit: (id, text) => Agents.queue("edit", id, text)
        }

        AgentImageStrip {
            id: strip
            x: 12
            anchors.top: queue.bottom
            anchors.topMargin: visible ? 8 : 0
            width: parent.width - 24
            height: visible ? implicitHeight : 0
            visible: images.length + adding > 0 || error.length > 0
            images: composer.controls.pending || []
            adding: composer.controls.adding || 0
            error: composer.controls.imageError || ""
            agentName: composer.agentName
            onRemoved: (id) => Agents.removeImage(id)
        }

        // The quote chips; on send their lines go before the words.
        Flow {
            id: quoteRow
            x: 12
            anchors.top: strip.bottom
            anchors.topMargin: visible ? 8 : 0
            width: parent.width - 24
            height: visible ? implicitHeight : 0
            visible: composer.quotes.length > 0
            spacing: 6
            Repeater {
                model: composer.quotes
                delegate: AgentQuoteChip {
                    required property var modelData
                    required property int index
                    path: modelData.path
                    first: modelData.first
                    last: modelData.last
                    onRemoved: Agents.quoteRemove(index)
                }
            }
        }

        Item {
            id: inputArea
            anchors.top: quoteRow.bottom
            anchors.topMargin: 8
            x: 12
            width: parent.width - 24
            height: Math.min(Math.max(input.contentHeight, 22), 140)
            Flickable {
                id: inputScroll
                anchors.fill: parent
                contentHeight: input.contentHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                TextEdit {
                    id: input
                    objectName: "input"
                    width: inputScroll.width
                    wrapMode: TextEdit.Wrap
                    readOnly: !composer.editable
                    opacity: composer.editable ? 1 : 0.7
                    textFormat: TextEdit.PlainText
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.body.size
                    color: Theme.surfaceContent
                    selectionColor: Theme.selection
                    selectedTextColor: Theme.surfaceContent
                    Accessible.name: placeholder.text
                    onTextChanged: {
                        if (!composer.settingText)
                            composer.typedFor = composer.ask.token || ""
                        composer.report(!composer.settingText)
                    }
                    onCursorPositionChanged: if (composer.popupOpen) composer.report(false)
                    onActiveFocusChanged: Agents.composerFocus(activeFocus)
                    Keys.onPressed: (event) => {
                        const enter = event.key === Qt.Key_Return || event.key === Qt.Key_Enter
                        if (composer.popupOpen) {
                            if (event.key === Qt.Key_Up || event.key === Qt.Key_Down) {
                                Agents.popupMove(event.key === Qt.Key_Up ? -1 : 1)
                                event.accepted = true
                                return
                            }
                            if ((enter && !(event.modifiers & (Qt.ShiftModifier | Qt.ControlModifier | Qt.AltModifier)))
                                || event.key === Qt.Key_Tab) {
                                composer.choose(-1)
                                event.accepted = true
                                return
                            }
                            if (event.key === Qt.Key_Escape) {
                                Agents.popupClose()
                                event.accepted = true
                                return
                            }
                        }
                        if (event.matches(StandardKey.Paste)) {
                            if (composer.controls.images === true && !composer.questionShown && Agents.pasteImage())
                                event.accepted = true
                            return
                        }
                        if ((event.key === Qt.Key_Up || event.key === Qt.Key_Down) && !composer.questionShown
                            && (input.length === 0 || composer.walkingHistory)) {
                            const json = Agents.historyStep(event.key === Qt.Key_Up)
                            if (json.length > 0) {
                                composer.walkingHistory = true
                                const prompt = JSON.parse(json).text
                                composer.put(prompt, prompt.length)
                                if (prompt.length === 0)
                                    composer.walkingHistory = false
                                event.accepted = true
                            }
                            return
                        }
                        if (!enter)
                            return
                        if (event.modifiers & Qt.ControlModifier) {
                            if (composer.canAllow)
                                composer.allow()
                            event.accepted = true
                        } else if (!(event.modifiers & Qt.ShiftModifier)) {
                            // While a question waits nothing is sent from here.
                            if (!composer.questionShown)
                                composer.submit((event.modifiers & Qt.AltModifier) !== 0)
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
                        id: placeholder
                        visible: input.text.length === 0
                        width: input.width
                        text: {
                            if (composer.questionShown) {
                                if (composer.answer.secret === true)
                                    return qsTr("Type it in the box above")
                                return composer.answer.other === true ? qsTr("Or type your own answer")
                                                                      : qsTr("Choose an answer above")
                            }
                            if (composer.sayWhy)
                                return qsTr("Say why, to deny it and tell %1 what to do instead").arg(composer.agentName)
                            if (composer.running && !composer.control)
                                return qsTr("You can reply when %1 stops.").arg(composer.agentName)
                            return qsTr("Reply to %1").arg(composer.agentName)
                        }
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                    // A command's arguments, after it, until you type them.
                    Txt {
                        readonly property rect end: input.positionToRectangle(input.length)
                        visible: (composer.controls.hint || "").length > 0 && !composer.popupOpen
                        x: end.x + 4
                        y: end.y
                        text: composer.controls.hint || ""
                        muted: true
                    }
                }
            }
            G7ScrollBar {
                flickable: inputScroll
                anchors.top: inputScroll.top
                anchors.bottom: inputScroll.bottom
                anchors.right: inputScroll.right
                anchors.rightMargin: -10
            }
        }

        // Images dropped on the box go with the next message.
        DropArea {
            anchors.fill: parent
            enabled: composer.controls.images === true && !composer.questionShown
            keys: ["text/uri-list"]
            onDropped: (drop) => {
                const urls = Array.from(drop.urls, (u) => String(u))
                if (urls.length > 0) {
                    Agents.addImages(JSON.stringify(urls))
                    drop.acceptProposedAction()
                }
            }
        }

        // ---- The footer ----
        Item {
            id: actions
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 4
            x: 6
            width: parent.width - 12
            height: 34
            Row {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4
                IconButton {
                    id: attach
                    objectName: "attach"
                    visible: composer.controls.images === true
                    anchors.verticalCenter: parent.verticalCenter
                    width: 30
                    height: 30
                    iconPath: Icons.photo
                    label: qsTr("Add image")
                    enabled: !composer.questionShown && !composer.busy
                    onClicked: composer.attachRequested()
                }
                // The model and effort, from the agent's own list.
                Rectangle {
                    id: modelControl
                    objectName: "model"
                    visible: composer.control
                    anchors.verticalCenter: parent.verticalCenter
                    width: modelRow.implicitWidth + 16
                    height: 28
                    radius: Theme.radiusSm
                    color: modelHover.hovered && composer.settingsOpen ? Theme.surfaceContainerHigh : "transparent"
                    opacity: composer.settingsOpen ? 1 : 0.6
                    Accessible.role: Accessible.Button
                    Accessible.name: modelText.text
                    Accessible.onPressAction: composer.openPicker()
                    Row {
                        id: modelRow
                        x: 8
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 4
                        Txt {
                            id: modelText
                            anchors.verticalCenter: parent.verticalCenter
                            text: (composer.picker.label && composer.picker.label.model.length > 0)
                                ? composer.picker.label.model : qsTr("Default")
                            role: "bodySmall"
                            weight: 600
                        }
                        Txt {
                            visible: composer.picker.label !== undefined && composer.picker.label.effort.length > 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: "· " + (composer.picker.label ? composer.picker.label.effort : "")
                            role: "bodySmall"
                            muted: true
                        }
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 12
                            height: 12
                            path: Icons.chevronDown
                            color: Theme.surfaceContentVariant
                        }
                    }
                    HoverHandler {
                        id: modelHover
                    }
                    MouseArea {
                        anchors.fill: parent
                        enabled: composer.settingsOpen
                        cursorShape: Qt.PointingHandCursor
                        onClicked: composer.openPicker()
                    }
                    // Why it rests.
                    Rectangle {
                        visible: modelHover.hovered && !composer.settingsOpen && composer.control
                        y: -height - 6
                        width: restText.implicitWidth + 16
                        height: restText.implicitHeight + 8
                        radius: Theme.radiusSm
                        color: Theme.surfaceContainerHighest
                        border.width: 1
                        border.color: Theme.outlineVariant
                        z: 30
                        Txt {
                            id: restText
                            anchors.centerIn: parent
                            text: composer.questionShown ? qsTr("Answer the question first.")
                                                         : qsTr("You can change it when %1 stops.").arg(composer.agentName)
                            role: "caption"
                        }
                    }
                }
                // The mode, only when it isn't the default.
                Rectangle {
                    objectName: "mode"
                    visible: composer.control && composer.mode !== "supervised"
                    anchors.verticalCenter: parent.verticalCenter
                    width: modeRow.implicitWidth + 14
                    height: 24
                    radius: Theme.radiusSm
                    color: Theme.surfaceContainerHigh
                    opacity: composer.settingsOpen ? 1 : 0.6
                    Row {
                        id: modeRow
                        x: 7
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 5
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13
                            height: 13
                            path: composer.mode === "plan" ? Icons.clipboardList : Icons.shield
                            color: Theme.surfaceContentVariant
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: composer.mode === "plan" ? qsTr("Plan") : qsTr("Asks before commands")
                            role: "caption"
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        enabled: composer.settingsOpen
                        cursorShape: Qt.PointingHandCursor
                        onClicked: composer.openPicker()
                    }
                }
            }
            Row {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Button {
                    id: stopButton
                    objectName: "stop"
                    visible: composer.running
                    anchors.verticalCenter: parent.verticalCenter
                    variant: "tonal"
                    size: "sm"
                    iconPath: Icons.stopSquare
                    text: qsTr("Stop")
                    busy: composer.busy
                    onClicked: composer.stop()
                }
                Button {
                    id: denySendButton
                    objectName: "denySend"
                    visible: composer.sayWhy && composer.hasText && !composer.questionShown
                    anchors.verticalCenter: parent.verticalCenter
                    size: "sm"
                    iconPath: Icons.send
                    text: qsTr("Deny and send")
                    enabled: composer.canDenySend
                    onClicked: composer.submit(false)
                }
                // Send; while it works, steer or queue; never while a question waits.
                Rectangle {
                    id: primary
                    objectName: "send"
                    visible: !composer.questionShown && !(composer.sayWhy && composer.hasText)
                             && (!composer.running || (composer.followUp && composer.hasText))
                    anchors.verticalCenter: parent.verticalCenter
                    width: 30
                    height: 30
                    radius: Theme.radiusSm
                    color: composer.canSend ? Theme.accent : Theme.surfaceContainerHigh
                    Accessible.role: Accessible.Button
                    Accessible.name: !composer.running ? qsTr("Send")
                        : composer.enterSteers ? qsTr("Send into this turn") : qsTr("Queue it")
                    Accessible.onPressAction: composer.submit(false)
                    Icon {
                        anchors.centerIn: parent
                        width: 16
                        height: 16
                        path: !composer.running ? composer.sendIcon
                            : composer.enterSteers ? composer.steerIcon : composer.queueIcon
                        color: composer.canSend ? Theme.accentContent : Theme.surfaceContentVariant
                    }
                    HoverHandler {
                        id: primaryHover
                    }
                    MouseArea {
                        anchors.fill: parent
                        enabled: composer.canSend
                        cursorShape: Qt.PointingHandCursor
                        onClicked: composer.submit(false)
                    }
                }
            }
            // While it works: what Enter and Alt+Enter do with these words.
            Rectangle {
                visible: primaryHover.hovered && composer.followUp
                anchors.right: parent.right
                anchors.bottom: parent.top
                anchors.bottomMargin: 6
                width: followColumn.implicitWidth + 20
                height: followColumn.implicitHeight + 14
                radius: Theme.radiusSm
                color: Theme.surfaceContainerHighest
                border.width: 1
                border.color: Theme.outlineVariant
                z: 30
                Column {
                    id: followColumn
                    x: 10
                    y: 7
                    spacing: 6
                    Repeater {
                        model: {
                            const steerHint = composer.agentId === "codex" ? qsTr("Codex reads it now.")
                                                                           : qsTr("Claude reads it at its next step.")
                            const queueHint = qsTr("Sent when %1 finishes.").arg(composer.agentName)
                            if (!composer.canSteer)
                                return [{ label: qsTr("Queue it"), keys: "↵", hint: queueHint }]
                            return composer.steerFirst
                                ? [{ label: qsTr("Send into this turn"), keys: "↵", hint: steerHint },
                                   { label: qsTr("Queue it instead"), keys: "Alt ↵", hint: queueHint }]
                                : [{ label: qsTr("Queue it"), keys: "↵", hint: queueHint },
                                   { label: qsTr("Send into this turn instead"), keys: "Alt ↵", hint: steerHint }]
                        }
                        delegate: Row {
                            id: followRow
                            required property var modelData
                            spacing: 8
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: followRow.modelData.label
                                role: "bodySmall"
                                weight: 600
                            }
                            Keycap {
                                anchors.verticalCenter: parent.verticalCenter
                                keys: followRow.modelData.keys
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: followRow.modelData.hint
                                role: "bodySmall"
                                muted: true
                            }
                        }
                    }
                }
            }
        }
    }

    // Up walks the prompts sent from this PC while the box is empty or walking.
    property bool walkingHistory: false
    onTextChanged: if (!settingText) walkingHistory = false

    // The folder it runs in; under "Deny and send", what Codex does with it.
    Item {
        id: caption
        anchors.top: box.bottom
        anchors.topMargin: 4
        width: parent.width
        height: 18
        Row {
            visible: composer.folder.length > 0
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 13
                height: 13
                path: Icons.folder
                color: Theme.surfaceContentVariant
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: composer.folder
                role: "caption"
                muted: true
            }
        }
        Txt {
            visible: composer.sayWhy && composer.hasText && composer.agentId === "codex"
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Codex stops and reads this next.")
            role: "caption"
            muted: true
        }
    }
}
