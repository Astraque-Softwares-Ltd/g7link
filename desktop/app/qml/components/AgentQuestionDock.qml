// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The agent asks, above the composer: one ask at a time ("1 of 3"), its
// options as a list (1–9 pick them), boxes when several may be picked, and
// "Something else", which is what you type in the composer. Next, then
// Send answers on the last; Dismiss sits under "…". A secret ask has a
// masked field of its own, never the composer, emptied whenever the ask
// changes. A question that arrived while you were typing waits, its buttons
// off, until you pause.
Item {
    id: dock
    // `Agents.question`, parsed.
    property var question: ({})
    property bool busy: false

    signal picked(int index)
    signal back
    signal next
    signal send
    signal secretTyped(string text)
    signal overflowRequested(real x, real y)

    readonly property var ask: question.ask || ({})
    readonly property var options: ask.options || []
    readonly property bool held: question.held === true
    readonly property int count: ask.count || 1
    readonly property int at: ask.at || 0
    readonly property bool last: ask.last === true
    readonly property bool ready: ask.ready === true && !held && !busy
    readonly property int clears: ask.clears || 0
    readonly property string token: question.token || ""
    // The index of "Something else" (just past the options).
    readonly property int otherIndex: options.length
    // A circled question mark (Tabler's help-circle, MIT, as Icons.qml credits).
    readonly property string questionIcon: "M3 12a9 9 0 1 0 18 0 9 9 0 1 0-18 0M12 17v.01M12 13.5a1.5 1.5 0 0 1 1-1.5 2.6 2.6 0 1 0-3-4"

    // A secret never outlives its ask: emptied on every change, and when
    // the dock goes.
    onClearsChanged: secretInput.text = ""
    onTokenChanged: secretInput.text = ""
    Component.onDestruction: secretInput.text = ""

    implicitHeight: card.implicitHeight

    Rectangle {
        id: card
        width: parent.width
        height: parent.height
        implicitHeight: header.height + scroll.contentHeight + buttons.height + 38
        radius: Theme.radiusMd
        color: Theme.surfaceContainerLow
        border.width: 1
        border.color: Theme.outlineVariant

        Item {
            id: header
            x: 12
            y: 10
            width: parent.width - 24
            height: 24
            Row {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: parent.left
                anchors.right: countText.left
                anchors.rightMargin: 8
                spacing: 8
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16
                    height: 16
                    path: dock.questionIcon
                    color: AgentPalette.needsYou
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("%1 asks").arg(dock.question.agentName || "")
                    weight: 600
                }
                Txt {
                    visible: (dock.ask.header || "").length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    text: "· " + (dock.ask.header || "")
                    role: "bodySmall"
                    muted: true
                }
            }
            Txt {
                id: countText
                visible: dock.count > 1
                anchors.right: more.left
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("%1 of %2").arg(dock.at + 1).arg(dock.count)
                role: "bodySmall"
                muted: true
            }
            IconButton {
                id: more
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: 26
                height: 26
                iconPath: Icons.more
                label: qsTr("More")
                onClicked: {
                    const p = more.mapToItem(null, more.width, more.height)
                    dock.overflowRequested(p.x, p.y)
                }
            }
        }

        Flickable {
            id: scroll
            x: 12
            anchors.top: header.bottom
            anchors.topMargin: 8
            anchors.bottom: buttons.top
            anchors.bottomMargin: 8
            width: parent.width - 24
            contentHeight: content.height
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            Column {
                id: content
                width: scroll.width
                spacing: 8
                Text {
                    width: parent.width
                    text: dock.ask.styled || ""
                    // Markup built by the app from escaped text; never the agent's.
                    textFormat: Text.StyledText
                    wrapMode: Text.Wrap
                    color: Theme.surfaceContent
                    linkColor: Theme.accent
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.body.size
                }
                Column {
                    width: parent.width
                    spacing: 2
                    Repeater {
                        model: dock.options
                        delegate: AgentAnswerRow {
                            required property var modelData
                            required property int index
                            width: content.width
                            styled: modelData.styled || ""
                            note: modelData.note
                            checked: modelData.checked
                            multi: dock.ask.multi === true
                            number: index + 1
                            enabled: !dock.held
                            onPicked: dock.picked(index)
                        }
                    }
                    AgentAnswerRow {
                        visible: dock.ask.other === true
                        width: content.width
                        label: qsTr("Something else")
                        note: qsTr("What you type below")
                        checked: dock.ask.otherOn === true
                        multi: dock.ask.multi === true
                        number: dock.otherIndex + 1
                        enabled: !dock.held
                        onPicked: dock.picked(dock.otherIndex)
                    }
                }
                // A secret: its own masked field, on this PC only.
                Column {
                    visible: dock.ask.secret === true
                    width: parent.width
                    spacing: 6
                    Rectangle {
                        width: parent.width
                        height: 34
                        radius: Theme.rowRadius
                        color: Theme.raised
                        border.width: secretInput.activeFocus ? 1 : 0
                        border.color: Theme.focusRing
                        TextInput {
                            id: secretInput
                            objectName: "secret"
                            x: 10
                            width: parent.width - 20
                            anchors.verticalCenter: parent.verticalCenter
                            echoMode: TextInput.Password
                            inputMethodHints: Qt.ImhSensitiveData | Qt.ImhNoPredictiveText | Qt.ImhNoAutoUppercase
                            enabled: !dock.held
                            clip: true
                            font.family: Theme.fontMono
                            font.pixelSize: Theme.typography.body.size
                            color: Theme.surfaceContent
                            selectionColor: Theme.selection
                            selectedTextColor: Theme.surfaceContent
                            Accessible.name: dock.ask.header || qsTr("Answer")
                            onTextChanged: dock.secretTyped(text)
                            onAccepted: if (dock.ready) dock.send()
                        }
                    }
                    Txt {
                        width: parent.width
                        text: qsTr("Goes to %1 only. G7 doesn't keep it.").arg(dock.question.agentName || "")
                        role: "caption"
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                }
            }
        }
        G7ScrollBar {
            flickable: scroll
            anchors.top: scroll.top
            anchors.bottom: scroll.bottom
            anchors.right: scroll.right
        }

        Item {
            id: buttons
            x: 12
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 10
            width: parent.width - 24
            height: 30
            Row {
                spacing: 8
                Button {
                    objectName: "back"
                    variant: "text"
                    size: "sm"
                    text: qsTr("Back")
                    enabled: dock.at > 0 && !dock.held
                    onClicked: dock.back()
                }
                Button {
                    objectName: "next"
                    size: "sm"
                    text: dock.last ? qsTr("Send answers") : qsTr("Next")
                    enabled: dock.ready
                    onClicked: dock.last ? dock.send() : dock.next()
                }
            }
            Row {
                visible: dock.held
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 13
                    height: 13
                    path: Icons.keyboard
                    color: AgentPalette.needsYou
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Arrived while you were typing")
                    role: "caption"
                    color: AgentPalette.needsYou
                }
            }
        }
    }
}
