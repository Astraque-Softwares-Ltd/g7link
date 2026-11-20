// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The three modes a thread can be in, no more: it asks before edits and
// commands (the default), before commands only, or it plans and changes
// nothing until you say so. Plan is there only when the agent can; Codex
// says why when it can't.
Column {
    id: modes
    // "supervised", "auto_edit" or "plan".
    property string mode: "supervised"
    property bool planAvailable: false
    // "claude" or "codex".
    property string agentId
    signal picked(string value)

    readonly property var rows: [
        { value: "supervised", label: qsTr("Asks before edits and commands"), note: qsTr("Your default"), enabled: true },
        { value: "auto_edit", label: qsTr("Asks before commands"), note: "", enabled: true },
        { value: "plan", label: qsTr("Plan: it changes nothing until you say so"),
          note: planAvailable ? "" : qsTr("Plan isn't available with this version of Codex."),
          enabled: planAvailable }
    ].filter((r) => r.value !== "plan" || r.enabled || agentId === "codex")

    spacing: 2

    Repeater {
        model: modes.rows
        delegate: AgentPickRow {
            required property var modelData
            width: modes.width
            label: modelData.label
            note: modelData.note
            checked: modes.mode === modelData.value
            enabled: modelData.enabled
            onPicked: modes.picked(modelData.value)
        }
    }
}
