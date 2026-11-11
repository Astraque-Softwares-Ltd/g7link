// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The call on the phone. Ringing: who's calling, Decline, Answer and Reply
// with a text. In a call: the clock, Mute, Speaker, Hold and Keypad, call
// volume and End. The sound stays on the phone; these change what it does.
Column {
    id: card
    spacing: 14

    // Ticks once a second for the call's clock.
    property real now: Date.now()
    Timer {
        interval: 1000
        repeat: true
        running: card.visible && PhoneCall.since > 0
        triggeredOnStart: true
        onTriggered: card.now = Date.now()
    }
    property bool keypad: false
    // Keys pressed on this call, as the phone's keypad would show them.
    property string pressed: ""
    Connections {
        target: PhoneCall
        function onActiveChanged() {
            card.keypad = false
            card.pressed = ""
        }
    }

    function clock(ms) {
        const s = Math.max(0, Math.floor(ms / 1000))
        const h = Math.floor(s / 3600)
        const m = Math.floor((s % 3600) / 60)
        const ss = String(s % 60).padStart(2, "0")
        return h > 0 ? h + ":" + String(m).padStart(2, "0") + ":" + ss : m + ":" + ss
    }
    function replyWithText() {
        Messages.startChat(PhoneCall.device, PhoneCall.number, PhoneCall.caller)
        AppController.currentPage = "messages"
    }

    // A flat button in a tint, for answering, declining and ending.
    component CallButton: FocusScope {
        id: action
        property string iconPath
        property string text
        property color fill
        signal clicked
        height: 38
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: text
        Accessible.onPressAction: action.clicked()
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                action.clicked()
                event.accepted = true
            }
        }
        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: action.fill
            border.width: Theme.focusVisible(action) ? 2 : 0
            border.color: Theme.focusRing
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                color: Theme.surface
                opacity: callTap.pressed ? 0.18 : (callHover.hovered ? 0.08 : 0)
            }
            Row {
                anchors.centerIn: parent
                spacing: 8
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16; height: 16
                    path: action.iconPath
                    color: Theme.surface
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: action.text
                    weight: 500
                    color: Theme.surface
                }
            }
        }
        HoverHandler { id: callHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { id: callTap; onTapped: action.clicked() }
    }

    // A square control with its name under it (Mute, Speaker, Hold, Keypad).
    component Control: FocusScope {
        id: control
        property string iconPath
        property string label
        property bool on: false
        signal clicked
        height: 54
        activeFocusOnTab: true
        Accessible.role: Accessible.CheckBox
        Accessible.name: label
        Accessible.checked: on
        Accessible.onPressAction: control.clicked()
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                control.clicked()
                event.accepted = true
            }
        }
        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: control.on ? Theme.accent : (controlHover.hovered ? Theme.selection : Theme.raised)
            border.width: Theme.focusVisible(control) ? 2 : 0
            border.color: Theme.focusRing
        }
        Column {
            anchors.centerIn: parent
            spacing: 4
            Icon {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 18; height: 18
                path: control.iconPath
                color: control.on ? Theme.accentContent : Theme.surfaceContent
            }
            Txt {
                anchors.horizontalCenter: parent.horizontalCenter
                role: "caption"
                color: control.on ? Theme.accentContent : Theme.surfaceContentVariant
                text: control.label
            }
        }
        HoverHandler { id: controlHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { onTapped: control.clicked() }
    }

    // ---- Who, and the answer ----
    Rectangle {
        width: parent.width
        height: callBox.implicitHeight + 32
        radius: Theme.radiusXl
        color: Theme.raised

        Column {
            id: callBox
            x: 16; y: 16
            width: parent.width - 32
            spacing: 14

            Item {
                width: parent.width
                height: heading.height
                Row {
                    spacing: 8
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14; height: 14
                        path: Icons.call
                        color: Theme.tintTeal
                    }
                    Txt {
                        id: heading
                        anchors.verticalCenter: parent.verticalCenter
                        text: PhoneCall.ringing ? qsTr("Incoming call") : qsTr("On a call")
                        role: "bodySmall"
                        color: Theme.tintTeal
                    }
                }
                Txt {
                    anchors.right: parent.right
                    role: "bodySmall"
                    color: PhoneCall.held ? Theme.tintAmber : Theme.surfaceContentVariant
                    text: PhoneCall.held ? qsTr("On hold")
                        : PhoneCall.since > 0 ? card.clock(card.now - PhoneCall.since) : ""
                }
            }

            Row {
                width: parent.width
                spacing: 12
                Avatar {
                    id: avatar
                    anchors.verticalCenter: parent.verticalCenter
                    width: 40; height: 40
                    // A number alone has no letter worth showing.
                    name: PhoneCall.number.length > 0 ? PhoneCall.caller : ""
                    iconPath: PhoneCall.number.length > 0 ? "" : Icons.person
                    hue: 0
                }
                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - avatar.width - parent.spacing
                    Txt {
                        width: parent.width
                        text: PhoneCall.caller
                        weight: 500
                        size: 15
                    }
                    Txt {
                        width: parent.width
                        visible: text.length > 0
                        role: "bodySmall"
                        muted: true
                        text: card.pressed.length > 0 ? card.pressed : PhoneCall.number
                        elide: Text.ElideLeft
                    }
                }
            }

            Grid {
                visible: PhoneCall.canEnd
                width: parent.width
                columns: PhoneCall.ringing ? 2 : 1
                columnSpacing: 8
                CallButton {
                    width: PhoneCall.ringing ? (parent.width - 8) / 2 : parent.width
                    iconPath: Icons.callEnd
                    text: PhoneCall.ringing ? qsTr("Decline") : qsTr("End call")
                    fill: Theme.tintRed
                    onClicked: PhoneCall.end()
                }
                CallButton {
                    visible: PhoneCall.ringing
                    width: (parent.width - 8) / 2
                    iconPath: Icons.call
                    text: qsTr("Answer")
                    fill: Theme.tintTeal
                    onClicked: PhoneCall.answer()
                }
            }
            TextAction {
                visible: PhoneCall.ringing && PhoneCall.number.length > 0
                anchors.horizontalCenter: parent.horizontalCenter
                text: qsTr("Reply with a text")
                onClicked: card.replyWithText()
            }
        }
    }

    // ---- What the phone does on the call ----
    SectionLabel {
        visible: PhoneCall.controls
        width: parent.width
        text: PhoneCall.ringing ? qsTr("After you answer") : qsTr("Call")
    }
    Row {
        id: controls
        visible: PhoneCall.controls
        width: parent.width
        spacing: 6
        readonly property int count: (PhoneCall.canHold || PhoneCall.held) ? 4 : 3
        readonly property real cell: (width - spacing * (count - 1)) / count
        Control {
            width: controls.cell
            iconPath: PhoneCall.muted ? Icons.micOff : Icons.mic
            label: PhoneCall.muted ? qsTr("Muted") : qsTr("Mute")
            on: PhoneCall.muted
            onClicked: PhoneCall.setMute(!PhoneCall.muted)
        }
        Control {
            width: controls.cell
            iconPath: Icons.speaker
            label: qsTr("Speaker")
            on: PhoneCall.speaker
            onClicked: PhoneCall.setSpeakerOn(!PhoneCall.speaker)
        }
        Control {
            visible: PhoneCall.canHold || PhoneCall.held
            width: controls.cell
            iconPath: Icons.pause
            label: PhoneCall.held ? qsTr("Resume") : qsTr("Hold")
            on: PhoneCall.held
            onClicked: PhoneCall.setHold(!PhoneCall.held)
        }
        Control {
            width: controls.cell
            iconPath: Icons.dialpad
            label: qsTr("Keypad")
            on: card.keypad
            onClicked: card.keypad = !card.keypad
        }
    }

    Grid {
        id: keys
        visible: PhoneCall.controls && card.keypad
        width: parent.width
        columns: 3
        columnSpacing: 6
        rowSpacing: 6
        Repeater {
            model: ["1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#"]
            delegate: Rectangle {
                id: key
                required property string modelData
                width: (keys.width - 12) / 3
                height: 36
                radius: Theme.rowRadius
                color: keyHover.hovered ? Theme.selection : Theme.raised
                border.width: Theme.focusVisible(key) ? 2 : 0
                border.color: Theme.focusRing
                activeFocusOnTab: true
                function press() {
                    PhoneCall.press(modelData)
                    card.pressed = (card.pressed + modelData).slice(-24)
                }
                Accessible.role: Accessible.Button
                Accessible.name: modelData
                Accessible.onPressAction: press()
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                        key.press()
                        event.accepted = true
                    }
                }
                Txt { anchors.centerIn: parent; size: 16; text: key.modelData }
                HoverHandler { id: keyHover; cursorShape: Qt.PointingHandCursor }
                TapHandler { onTapped: key.press() }
            }
        }
    }

    Txt {
        width: parent.width
        visible: !PhoneCall.controls && PhoneCall.canEnd
        role: "bodySmall"
        muted: true
        wrapMode: Text.WordWrap
        text: qsTr("Mute, speaker and keypad need Elevated (Android 12 or later)")
    }

    // ---- Call volume (on the phone) ----
    Row {
        visible: PhoneCall.canEnd && !PhoneCall.ringing
        spacing: 4
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            role: "bodySmall"
            muted: true
            text: qsTr("Call volume")
        }
        IconButton {
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.volumeDown
            label: qsTr("Call volume down")
            onClicked: PhoneCall.volume(false)
        }
        IconButton {
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.volumeUp
            label: qsTr("Call volume up")
            onClicked: PhoneCall.volume(true)
        }
    }

    // ---- Bluetooth HFP: the call's sound on this PC ----
    Item {
        visible: PhoneCall.hfpDetail.length > 0
        width: parent.width
        height: Math.max(hfpText.implicitHeight, hfpAction.height)
        Txt {
            id: hfpText
            anchors.left: parent.left
            anchors.right: hfpAction.visible ? hfpAction.left : parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            role: "bodySmall"
            color: PhoneCall.hfpConnected ? Theme.tintTeal : Theme.surfaceContentVariant
            wrapMode: Text.WordWrap
            text: PhoneCall.hfpDetail
        }
        TextAction {
            id: hfpAction
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            visible: !PhoneCall.hfpConnected
            text: PhoneCall.hfpAvailable ? qsTr("Connect PC audio") : qsTr("Bluetooth settings")
            onClicked: {
                if (PhoneCall.hfpAvailable)
                    PhoneCall.connectPcAudio()
                else
                    PhoneCall.openBluetoothSettings()
            }
        }
    }
    Txt {
        width: parent.width
        visible: PhoneCall.canEnd && !PhoneCall.hfpConnected
        role: "bodySmall"
        muted: true
        text: qsTr("Sound stays on the phone")
    }
}
