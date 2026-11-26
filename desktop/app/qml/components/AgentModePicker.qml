// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The modes the thread's agent offers, in its CLI's order and words (the
// model control's side column): each its name, the line under it, a check
// on the one in use; one the agent can't use now says why.
Column {
    id: modes
    // `picker.modes`: [{ id, name, line, on, why, checked, warn }].
    property var rows: []
    signal picked(string value)

    spacing: 2

    Repeater {
        model: modes.rows
        delegate: AgentPickRow {
            required property var modelData
            width: modes.width
            label: modelData.name
            note: modelData.on === false && (modelData.why || "").length > 0 ? modelData.why : modelData.line
            checked: modelData.checked === true
            enabled: modelData.on !== false
            onPicked: modes.picked(modelData.id)
        }
    }
}
