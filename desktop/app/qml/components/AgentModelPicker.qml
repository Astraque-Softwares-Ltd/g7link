// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The open thread's model control: the models the agent's own CLI reported
// on this PC by their exact names ("Opus 5", the CLI's default tagged
// "Default"; short names first, the rest under "More models", then a name
// you type), the chosen model's thinking (Claude) or reasoning (Codex)
// levels as the CLI names them, and the modes. Before the CLI has reported
// a list: the agent's default and a typed name. Over the page, it closes on
// Esc or a click outside; each pick applies to the thread's next message.
Item {
    id: panelRoot
    // `Agents.composer.picker`, parsed.
    property var picker: ({})
    property bool opened: false
    property real anchorX: 0
    property real anchorY: 0
    // This PC's name, for "As Claude reported them on …".
    property string pcName

    property bool showMore: false
    property bool typing: false

    readonly property var shortRows: picker.short || []
    readonly property var moreRows: picker.more || []
    readonly property var efforts: picker.efforts || []
    readonly property bool known: picker.known === true
    readonly property string agentName: picker.agentName || ""
    readonly property bool custom: (picker.custom || "").length > 0
    // A row's name: "" is the CLI's default when the CLI doesn't say which model it is.
    function rowName(row) {
        return row.name.length > 0 ? row.name : qsTr("%1's default").arg(agentName)
    }

    function openAt(x, y) {
        anchorX = x
        anchorY = y
        showMore = moreRows.some((m) => m.checked)
        typing = custom || !known
        customField.text = picker.custom || ""
        opened = true
        keyScope.forceActiveFocus()
    }
    function close() { opened = false }
    function pick(what, value) {
        Agents.pick(false, what, value)
    }

    anchors.fill: parent
    visible: opacity > 0
    enabled: opened
    opacity: opened ? 1 : 0
    z: 150
    Behavior on opacity {
        NumberAnimation {
            duration: panelRoot.opened ? Theme.smallIn : Theme.smallOut
            easing.type: Easing.BezierSpline
            easing.bezierCurve: panelRoot.opened ? Theme.easeEnter : Theme.easeExit
        }
    }

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        onPressed: panelRoot.close()
        onWheel: (wheel) => wheel.accepted = true
    }

    Rectangle {
        id: panel
        width: Math.min(580, panelRoot.width - 24)
        height: Math.min(body.height + footer.height + 16, panelRoot.height - 24)
        x: Math.max(12, Math.min(panelRoot.width - width - 12, panelRoot.anchorX))
        y: Math.max(12, panelRoot.anchorY - height - 6)
        radius: Theme.radiusXl
        color: Theme.surface
        border.width: 1
        border.color: Theme.line
        clip: true

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
        }

        FocusScope {
            id: keyScope
            anchors.fill: parent
            Keys.onEscapePressed: panelRoot.close()

            Row {
                id: body
                x: 8
                y: 8
                width: parent.width - 16
                spacing: 10

                // ---- Models ----
                Column {
                    id: modelColumn
                    width: (body.width - 11) / 2
                    spacing: 2
                    Txt {
                        x: 8
                        height: 24
                        verticalAlignment: Text.AlignVCenter
                        text: panelRoot.picker.agent === "codex" ? qsTr("Codex models") : qsTr("Claude models")
                        role: "label"
                        muted: true
                    }
                    Repeater {
                        model: panelRoot.shortRows
                        delegate: AgentPickRow {
                            required property var modelData
                            width: modelColumn.width
                            label: panelRoot.rowName(modelData)
                            note: modelData.note
                            tag: modelData.isDefault && modelData.name.length > 0 ? qsTr("Default") : ""
                            checked: modelData.checked && !panelRoot.custom
                            onPicked: {
                                panelRoot.typing = false
                                panelRoot.pick("model", modelData.id)
                            }
                        }
                    }
                    AgentPickRow {
                        visible: panelRoot.moreRows.length > 0
                        width: modelColumn.width
                        label: qsTr("More models")
                        tag: String(panelRoot.moreRows.length)
                        opens: true
                        onPicked: panelRoot.showMore = !panelRoot.showMore
                    }
                    Repeater {
                        model: panelRoot.showMore ? panelRoot.moreRows : []
                        delegate: AgentPickRow {
                            required property var modelData
                            width: modelColumn.width
                            label: panelRoot.rowName(modelData)
                            note: modelData.note
                            checked: modelData.checked && !panelRoot.custom
                            onPicked: {
                                panelRoot.typing = false
                                panelRoot.pick("model", modelData.id)
                            }
                        }
                    }
                    AgentPickRow {
                        visible: panelRoot.known
                        width: modelColumn.width
                        label: qsTr("Other…")
                        checked: panelRoot.custom
                        onPicked: {
                            panelRoot.typing = true
                            customField.input.forceActiveFocus()
                        }
                    }
                    Field {
                        id: customField
                        visible: panelRoot.typing
                        x: 4
                        width: modelColumn.width - 8
                        placeholder: qsTr("Model name, as you'd give it to %1").arg(panelRoot.agentName)
                        onAccepted: {
                            if (customField.text.trim().length > 0)
                                panelRoot.pick("custom", customField.text.trim())
                        }
                    }
                }

                Rectangle {
                    width: 1
                    height: Math.max(modelColumn.height, sideColumn.height)
                    color: Theme.line
                }

                // ---- Effort and mode ----
                Column {
                    id: sideColumn
                    width: (body.width - 11) / 2
                    spacing: 2
                    Txt {
                        visible: panelRoot.efforts.length > 0
                        x: 8
                        height: 24
                        verticalAlignment: Text.AlignVCenter
                        text: panelRoot.picker.effortWord === "reasoning" ? qsTr("Reasoning") : qsTr("Thinking")
                        role: "label"
                        muted: true
                    }
                    Repeater {
                        model: panelRoot.efforts
                        delegate: AgentPickRow {
                            required property var modelData
                            width: sideColumn.width
                            label: modelData.label
                            tag: modelData.isDefault ? qsTr("Default") : ""
                            checked: modelData.checked
                            onPicked: panelRoot.pick("effort", modelData.value)
                        }
                    }
                    Txt {
                        x: 8
                        height: 24
                        verticalAlignment: Text.AlignVCenter
                        text: qsTr("Mode")
                        role: "label"
                        muted: true
                    }
                    AgentModePicker {
                        width: sideColumn.width
                        rows: panelRoot.picker.modes || []
                        onPicked: (value) => panelRoot.pick("mode", value)
                    }
                }
            }

            Item {
                id: footer
                anchors.bottom: parent.bottom
                width: parent.width
                height: 34
                Rectangle {
                    width: parent.width
                    height: 1
                    color: Theme.line
                }
                Txt {
                    x: 16
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Applies to the next message.")
                    role: "caption"
                    muted: true
                }
                Txt {
                    visible: panelRoot.known
                    anchors.right: parent.right
                    anchors.rightMargin: 16
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("As %1 reported them on %2").arg(panelRoot.agentName).arg(panelRoot.pcName)
                    role: "caption"
                    muted: true
                }
            }
        }
    }
}
