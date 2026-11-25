// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The new thread's "Run with": up to three quick picks, each the agent's
// logo, its name, the exact model and the effort ("Claude · Opus 5 ·
// High"), with where it comes from ("Last used here", "Used in site",
// "Claude's default"), then "Other…", which opens here: the agent (every
// one, those that can't start greyed with why), the model (its exact name,
// the rest under "More…", or one you type) and Claude's thinking or Codex's
// reasoning ("Auto" first when the model has no default). A choice made
// there is "Your choice". Without the host's model control only the agent
// is offered.
Column {
    id: runWith
    // Agents.newPicker, parsed.
    property var picker: ({})
    // This PC's name, for "Codex is signed out on <PC>.".
    property string pcName
    // The rows behind "More…", as PopupMenu actions, at (x, y) in the window.
    signal menu(real x, real y, var actions)

    property bool otherOpen: false
    property bool typing: false

    readonly property bool control: picker.control === true
    readonly property var picks: picker.picks || []
    readonly property var agents: picker.agents || []
    readonly property var leftOut: picker.leftOut || []
    readonly property string agentName: picker.agentName || ""
    readonly property bool thinking: picker.effortWord === "thinking"
    readonly property var effortOptions: {
        const efforts = picker.efforts || []
        const auto = picker.effortAuto === true ? [{ value: "", label: qsTr("Auto") }] : []
        return auto.concat(efforts.map((e) => ({ value: e.value, label: e.label })))
    }
    readonly property string effortValue: {
        const efforts = picker.efforts || []
        const set = efforts.find((e) => e.checked === true)
        return set ? set.value : ""
    }
    readonly property string defaultEffort: ((picker.efforts || []).find((e) => e.isDefault === true) || {}).label || ""

    // A model's name as rows give it: "" is the CLI's default with no version reported.
    function modelName(name) {
        return name.length > 0 ? name : qsTr("%1's default").arg(agentName)
    }
    // Why an agent can't start a thread, in a line.
    function stateLine(agent) {
        switch (agent.state) {
        case "signed_out": return qsTr("%1 is signed out on %2.").arg(agent.name).arg(pcName)
        case "missing": return qsTr("%1 isn't installed on %2.").arg(agent.name).arg(pcName)
        case "problem": return qsTr("%1 isn't ready on %2.").arg(agent.name).arg(pcName)
        default: return ""
        }
    }
    // Picks a quick pick (Alt+1..3).
    function choose(index) {
        const pick = picks[index]
        if (pick && pick.from !== "yours")
            Agents.pick(true, "pick", String(pick.index))
    }

    spacing: 8
    onPickerChanged: if ((picker.custom || "").length === 0 && !customField.input.activeFocus) typing = false

    Txt {
        visible: runWith.control
        text: qsTr("Run with")
        role: "label"
        muted: true
    }

    Flow {
        id: pillFlow
        visible: runWith.control
        width: parent.width
        spacing: 6
        Repeater {
            model: runWith.picks
            delegate: FocusScope {
                id: pill
                required property var modelData
                required property int index
                readonly property bool checked: modelData.checked === true
                readonly property string sub: modelData.from === "yours" ? qsTr("Your choice")
                    : modelData.delisted === true ? qsTr("Not in %1's list now").arg(modelData.agentName)
                    : modelData.from === "here" ? qsTr("Last used here")
                    : modelData.from === "folder" ? qsTr("Used in %1").arg(modelData.folder)
                    : modelData.name.length > 0 ? qsTr("%1's default").arg(modelData.agentName) : ""
                readonly property real maxText: pillFlow.width - 20 - 20
                width: Math.min(pillFlow.width, Math.max(labelRow.implicitWidth, subText.implicitWidth) + 20)
                height: 44
                activeFocusOnTab: true
                Accessible.role: Accessible.RadioButton
                Accessible.name: [agentPart.text + modelPart.text + effortPart.text, sub].filter((s) => s.length > 0).join(", ")
                Accessible.checked: checked
                Accessible.onPressAction: runWith.choose(index)
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                        runWith.choose(pill.index)
                        event.accepted = true
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    radius: Theme.radiusMd
                    color: pill.checked ? Theme.activeFill : (pillMouse.containsMouse ? Theme.raised : "transparent")
                    border.width: 1
                    border.color: pill.checked ? Theme.accent : Theme.line
                    Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
                }
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: -3
                    radius: Theme.radiusMd + 3
                    color: "transparent"
                    border.width: 2
                    border.color: Theme.focusRing
                    visible: Theme.focusVisible(pill)
                }
                Row {
                    id: labelRow
                    x: 10
                    y: 6
                    height: 18
                    spacing: 0
                    AgentMark {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        agent: pill.modelData.agent
                    }
                    Item { width: 6; height: 1 }
                    Txt {
                        id: agentPart
                        anchors.verticalCenter: parent.verticalCenter
                        // No version known for the default: "Claude's default" says it all.
                        text: pill.modelData.name.length > 0 ? pill.modelData.agentName + " · " : ""
                        role: "bodySmall"
                        weight: 600
                        color: pill.checked ? Theme.accent : Theme.surfaceContent
                    }
                    // The model's name, shortened in its middle when the pill can't hold it.
                    TextMetrics {
                        id: modelFit
                        font: modelPart.font
                        elide: Text.ElideMiddle
                        elideWidth: Math.max(0, pill.maxText - agentPart.implicitWidth - effortPart.implicitWidth)
                        text: pill.modelData.name.length > 0 ? pill.modelData.name : qsTr("%1's default").arg(pill.modelData.agentName)
                    }
                    Txt {
                        id: modelPart
                        anchors.verticalCenter: parent.verticalCenter
                        text: modelFit.elidedText
                        role: "bodySmall"
                        weight: 600
                        elide: Text.ElideNone
                        color: agentPart.color
                    }
                    Txt {
                        id: effortPart
                        anchors.verticalCenter: parent.verticalCenter
                        text: pill.modelData.effort.length > 0 ? " · " + pill.modelData.effort : ""
                        role: "bodySmall"
                        weight: 600
                        color: agentPart.color
                    }
                }
                Txt {
                    id: subText
                    x: 30
                    y: 24
                    width: Math.min(implicitWidth, pill.width - 40)
                    text: pill.sub
                    role: "caption"
                    color: pill.modelData.delisted === true && pill.modelData.from !== "yours"
                        ? Theme.tintAmber : Theme.surfaceContentVariant
                }
                MouseArea {
                    id: pillMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: runWith.choose(pill.index)
                }
            }
        }
        // Other…: opens the agent, model and effort here.
        FocusScope {
            id: otherPill
            width: otherRow.implicitWidth + 24
            height: 44
            activeFocusOnTab: true
            Accessible.role: Accessible.Button
            Accessible.name: qsTr("Other…")
            Accessible.onPressAction: runWith.otherOpen = !runWith.otherOpen
            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                    runWith.otherOpen = !runWith.otherOpen
                    event.accepted = true
                }
            }
            Rectangle {
                anchors.fill: parent
                radius: Theme.radiusMd
                color: runWith.otherOpen || otherMouse.containsMouse ? Theme.raised : "transparent"
                border.width: 1
                border.color: Theme.line
            }
            Rectangle {
                anchors.fill: parent
                anchors.margins: -3
                radius: Theme.radiusMd + 3
                color: "transparent"
                border.width: 2
                border.color: Theme.focusRing
                visible: Theme.focusVisible(otherPill)
            }
            Row {
                id: otherRow
                anchors.centerIn: parent
                spacing: 4
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Other…")
                    role: "bodySmall"
                    weight: 600
                }
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                    path: runWith.otherOpen ? Icons.chevronUp : Icons.chevronDown
                    color: Theme.surfaceContentVariant
                }
            }
            MouseArea {
                id: otherMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: runWith.otherOpen = !runWith.otherOpen
            }
        }
    }

    // A pick left out: its agent can't start a thread now.
    Repeater {
        model: runWith.agents.filter((a) => runWith.leftOut.includes(a.id))
        delegate: Txt {
            required property var modelData
            width: runWith.width
            text: runWith.stateLine(modelData)
            role: "bodySmall"
            color: Theme.tintAmber
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }

    // ---- Other… (always shown, as the agent alone, without the model control) ----
    Column {
        visible: runWith.otherOpen || !runWith.control
        width: parent.width
        spacing: 12
        topPadding: runWith.control ? 4 : 0

        Column {
            width: parent.width
            spacing: 6
            Txt {
                text: qsTr("Agent")
                role: "label"
                muted: true
            }
            Flow {
                width: parent.width
                spacing: 6
                Repeater {
                    model: runWith.agents
                    delegate: FocusScope {
                        id: agentButton
                        required property var modelData
                        readonly property bool ready: modelData.state === "ready"
                        readonly property bool checked: runWith.picker.agent === modelData.id
                        width: agentRow.implicitWidth + 24
                        height: 30
                        enabled: ready
                        opacity: ready ? 1 : 0.38
                        activeFocusOnTab: ready
                        Accessible.role: Accessible.RadioButton
                        Accessible.name: modelData.name
                        Accessible.description: runWith.stateLine(modelData)
                        Accessible.checked: checked
                        Accessible.onPressAction: Agents.newForm(modelData.id, runWith.picker.folder || "")
                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                Agents.newForm(agentButton.modelData.id, runWith.picker.folder || "")
                                event.accepted = true
                            }
                        }
                        Rectangle {
                            anchors.fill: parent
                            radius: Theme.radiusSm
                            color: agentButton.checked ? Theme.activeFill : (agentMouse.containsMouse ? Theme.raised : "transparent")
                            border.width: 1
                            border.color: agentButton.checked ? Theme.accent : Theme.line
                        }
                        Rectangle {
                            anchors.fill: parent
                            anchors.margins: -3
                            radius: Theme.radiusSm + 3
                            color: "transparent"
                            border.width: 2
                            border.color: Theme.focusRing
                            visible: Theme.focusVisible(agentButton)
                        }
                        Row {
                            id: agentRow
                            anchors.centerIn: parent
                            spacing: 6
                            AgentMark {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                                agent: agentButton.modelData.id
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: agentButton.modelData.name
                                role: "bodySmall"
                                color: agentButton.checked ? Theme.accent : Theme.surfaceContent
                            }
                        }
                        MouseArea {
                            id: agentMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: Agents.newForm(agentButton.modelData.id, runWith.picker.folder || "")
                        }
                    }
                }
            }
            Repeater {
                model: runWith.agents.filter((a) => a.state !== "ready" && !runWith.leftOut.includes(a.id))
                delegate: Txt {
                    required property var modelData
                    width: runWith.width
                    text: runWith.stateLine(modelData)
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
            }
        }

        // The model: the exact names first, the rest behind More…, or one typed.
        Column {
            visible: runWith.control && (runWith.picker.agent || "").length > 0
            width: parent.width
            spacing: 6
            Txt {
                text: qsTr("Model")
                role: "label"
                muted: true
            }
            Flow {
                id: chipFlow
                width: parent.width
                spacing: 6
                Repeater {
                    model: runWith.picker.chips || []
                    delegate: Chip {
                        id: modelChip
                        required property var modelData
                        width: Math.min(implicitWidth, chipFlow.width)
                        interactive: true
                        selected: modelData.checked === true
                        text: {
                            const name = runWith.modelName(modelData.name)
                            // The CLI's default, named: "Opus 5 · Default".
                            return modelData.isDefault === true && modelData.name.length > 0 ? name + " · " + qsTr("Default") : name
                        }
                        onClicked: {
                            runWith.typing = false
                            if (modelChip.modelData.custom !== true)
                                Agents.pick(true, "model", modelChip.modelData.id)
                        }
                    }
                }
                Chip {
                    id: moreChip
                    visible: (runWith.picker.menu || []).length > 0
                    interactive: true
                    text: qsTr("More…")
                    onClicked: {
                        const p = moreChip.mapToItem(null, 0, moreChip.height)
                        runWith.menu(p.x, p.y, (runWith.picker.menu || []).map((m) => ({
                            label: runWith.modelName(m.name),
                            detail: m.note,
                            checked: m.checked === true && (runWith.picker.custom || "").length === 0,
                            run: () => {
                                runWith.typing = false
                                Agents.pick(true, "model", m.id)
                            }
                        })))
                    }
                }
                Chip {
                    interactive: true
                    selected: runWith.typing
                    text: qsTr("Type a model…")
                    onClicked: {
                        runWith.typing = true
                        customField.text = runWith.picker.custom || ""
                        customField.input.forceActiveFocus()
                    }
                }
            }
            Field {
                id: customField
                visible: runWith.typing || runWith.picker.known !== true
                width: Math.min(parent.width, 360)
                placeholder: qsTr("Model name, as you'd give it to %1").arg(runWith.agentName)
                onAccepted: {
                    if (customField.text.trim().length > 0)
                        Agents.pick(true, "custom", customField.text.trim())
                }
            }
        }

        // Claude's thinking, Codex's reasoning: Auto (the model's own) first when it has no default.
        Column {
            visible: runWith.control && runWith.effortOptions.length > 0
            width: parent.width
            spacing: 6
            Txt {
                text: runWith.thinking ? qsTr("Thinking") : qsTr("Reasoning")
                role: "label"
                muted: true
            }
            Segmented {
                options: runWith.effortOptions
                value: runWith.effortValue
                onPicked: (value) => Agents.pick(true, "effort", value)
            }
            Txt {
                visible: runWith.defaultEffort.length > 0
                text: qsTr("Default: %1").arg(runWith.defaultEffort)
                role: "bodySmall"
                muted: true
            }
        }
    }
}
