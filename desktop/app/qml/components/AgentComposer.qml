// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The reply box under a thread. Enter sends; Shift+Enter starts a new line.
// While a request waits and the host takes words, what you type denies it
// and tells the agent what to do instead ("Deny and send"), but only for the
// request it was typed under, and never while that one is held. Ctrl+Enter
// allows the request only while the box is empty, so a habit of Ctrl+Enter
// to send can never allow a command. Stop sits beside send while a turn runs
// or waits.
Item {
    id: composer
    // The request the dock shows (`Agents.ask`, parsed; `{}` for none).
    property var ask: ({})
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

    signal send(string text)
    signal denySend(string token, string text)
    signal allow
    signal stop

    readonly property bool asking: (ask.token || "").length > 0
    // The request the words were typed under ("" for none).
    property string typedFor: ""
    // The page sets the text (not the user).
    property bool settingText: false
    readonly property alias text: input.text
    readonly property bool hasText: input.text.trim().length > 0
    // Words now deny the shown request and tell the agent why.
    readonly property bool sayWhy: asking && canSayWhy
    readonly property bool canSend: !running && hasText && !busy
    readonly property bool canDenySend: sayWhy && hasText && typedFor === (ask.token || "")
        && ask.held !== true && !busy
    readonly property bool canAllow: asking && ask.held !== true && !busy && !hasText
    readonly property bool editable: !running || sayWhy

    function clear() {
        settingText = true
        input.text = ""
        settingText = false
    }
    function submit() {
        if (sayWhy && hasText) {
            if (canDenySend) {
                composer.denySend(ask.token, input.text)
                clear()
            }
            return
        }
        if (canSend) {
            composer.send(input.text)
            clear()
        }
    }
    function focusInput() {
        input.forceActiveFocus()
    }

    implicitHeight: box.height + caption.height + 4

    Rectangle {
        id: box
        width: parent.width
        height: Math.min(Math.max(input.contentHeight + 18, 38), 150) + actions.height
        radius: Theme.rowRadius
        color: Theme.raised
        border.width: input.activeFocus ? 1 : 0
        border.color: Theme.focusRing
        Flickable {
            id: inputScroll
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: actions.top
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            anchors.topMargin: 9
            contentHeight: input.contentHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            G7ScrollBar {
                flickable: inputScroll
                parent: inputScroll.parent
                anchors.top: inputScroll.top
                anchors.bottom: inputScroll.bottom
                anchors.right: inputScroll.right
                anchors.rightMargin: -10
            }
            TextEdit {
                id: input
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
                    if (composer.settingText)
                        return
                    composer.typedFor = composer.ask.token || ""
                    Agents.typed()
                }
                onActiveFocusChanged: Agents.composerFocus(activeFocus)
                Keys.onPressed: (event) => {
                    if (event.key !== Qt.Key_Return && event.key !== Qt.Key_Enter)
                        return
                    if (event.modifiers & Qt.ControlModifier) {
                        if (composer.canAllow)
                            composer.allow()
                        event.accepted = true
                    } else if (!(event.modifiers & Qt.ShiftModifier)) {
                        composer.submit()
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
                    text: composer.sayWhy
                        ? qsTr("Say why, to deny it and tell %1 what to do instead").arg(composer.agentName)
                        : composer.running ? qsTr("You can reply when %1 stops.").arg(composer.agentName)
                                           : qsTr("Message %1").arg(composer.agentName)
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
            }
        }
        Row {
            id: actions
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 6
            height: 30
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
                visible: composer.sayWhy && composer.hasText
                anchors.verticalCenter: parent.verticalCenter
                size: "sm"
                iconPath: Icons.send
                text: qsTr("Deny and send")
                enabled: composer.canDenySend
                onClicked: composer.submit()
            }
            IconButton {
                id: sendButton
                objectName: "send"
                visible: !(composer.sayWhy && composer.hasText)
                anchors.verticalCenter: parent.verticalCenter
                width: 30
                height: 30
                iconPath: Icons.send
                label: qsTr("Send")
                enabled: composer.canSend
                onClicked: composer.submit()
            }
        }
    }
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
