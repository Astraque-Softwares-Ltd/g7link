// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Setup's Scripts on a folder's card (§X5.1): its scripts (name, command,
// edit, remove, up and down), "Add a script", and "Phones can run these".
// The whole list goes to the host on each change; the host checks it and its
// reason words show here as copy. Nothing is read from the folder itself.
Column {
    id: area
    property string folderId
    // The folder's `scripts` in Agents.setup: { list: [{ id, name, command }], phones, saving, refused }.
    property var scripts: ({})

    readonly property var list: scripts.list || []
    readonly property bool full: list.length >= 12
    // The row being edited (-1: none).
    property int editing: -1

    readonly property string refusedText: {
        switch (scripts.refused || "") {
        case "": return ""
        case "name": return qsTr("Give it a name.")
        case "duplicate": return qsTr("A script with that name exists.")
        case "command": return qsTr("Type the command on one line.")
        case "many": return qsTr("Up to 12 scripts.")
        case "denied": return qsTr("That is done from G7 Link on this PC.")
        default: return qsTr("Something went wrong. Try again.")
        }
    }

    function save(rows) {
        Agents.scriptsSave(area.folderId, JSON.stringify(rows.map((r) => r.id ? { id: r.id, name: r.name, command: r.command }
                                                                              : { name: r.name, command: r.command })))
    }
    function moved(from, to) {
        const rows = area.list.slice()
        if (to < 0 || to >= rows.length)
            return
        const [row] = rows.splice(from, 1)
        rows.splice(to, 0, row)
        save(rows)
    }
    function removed(index) {
        save(area.list.filter((_, i) => i !== index))
        area.editing = -1
    }
    function edited(index, name, command) {
        save(area.list.map((r, i) => i === index ? { id: r.id, name: name.trim(), command: command.trim() } : r))
        area.editing = -1
    }
    function added(name, command) {
        save(area.list.concat([{ name: name.trim(), command: command.trim() }]))
    }

    spacing: 4

    Repeater {
        model: area.list
        delegate: Item {
            id: scriptRow
            required property var modelData
            required property int index
            readonly property bool open: area.editing === scriptRow.index
            width: area.width
            height: scriptRow.open ? editBox.height + 8 : Math.max(40, rowTexts.implicitHeight + 12)

            Column {
                id: rowTexts
                visible: !scriptRow.open
                x: 0
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - rowButtons.width - 8
                spacing: 1
                Txt {
                    width: parent.width
                    text: scriptRow.modelData.name
                    role: "bodySmall"
                    weight: 500
                    maximumLineCount: 1
                }
                Txt {
                    width: parent.width
                    text: scriptRow.modelData.command
                    role: "code"
                    size: Theme.typography.caption.size
                    muted: true
                    maximumLineCount: 1
                    elide: Text.ElideMiddle
                }
            }
            Row {
                id: rowButtons
                visible: !scriptRow.open
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                IconButton {
                    iconPath: Icons.chevronUp
                    iconSize: 14
                    label: qsTr("Up")
                    enabled: scriptRow.index > 0 && area.scripts.saving !== true
                    onClicked: area.moved(scriptRow.index, scriptRow.index - 1)
                }
                IconButton {
                    iconPath: Icons.chevronDown
                    iconSize: 14
                    label: qsTr("Down")
                    enabled: scriptRow.index < area.list.length - 1 && area.scripts.saving !== true
                    onClicked: area.moved(scriptRow.index, scriptRow.index + 1)
                }
                IconButton {
                    iconPath: Icons.edit
                    iconSize: 14
                    label: qsTr("Edit")
                    enabled: area.scripts.saving !== true
                    onClicked: area.editing = scriptRow.index
                }
                IconButton {
                    iconPath: Icons.trash
                    iconSize: 14
                    label: qsTr("Remove")
                    enabled: area.scripts.saving !== true
                    onClicked: area.removed(scriptRow.index)
                }
            }
            // Editing in place: Name, Command, Save.
            Column {
                id: editBox
                visible: scriptRow.open
                width: parent.width
                spacing: 6
                Field {
                    id: editName
                    width: parent.width
                    placeholder: qsTr("Name")
                    text: scriptRow.modelData.name
                }
                Field {
                    id: editCommand
                    width: parent.width
                    placeholder: qsTr("Command")
                    text: scriptRow.modelData.command
                    onAccepted: if (saveButton.enabled) saveButton.clicked()
                }
                Row {
                    spacing: 8
                    Button {
                        id: saveButton
                        size: "sm"
                        text: qsTr("Save")
                        busy: area.scripts.saving === true
                        enabled: editName.text.trim().length > 0 && editCommand.text.trim().length > 0
                        onClicked: area.edited(scriptRow.index, editName.text, editCommand.text)
                    }
                    Button {
                        size: "sm"
                        variant: "text"
                        text: qsTr("Cancel")
                        onClicked: area.editing = -1
                    }
                }
            }
        }
    }

    // ---- Add a script ----
    Txt {
        visible: area.full
        width: parent.width
        text: qsTr("Up to 12 scripts.")
        role: "bodySmall"
        muted: true
    }
    Column {
        visible: !area.full
        width: parent.width
        spacing: 6
        Txt {
            text: qsTr("Add a script")
            role: "label"
            muted: true
        }
        Field {
            id: newName
            width: parent.width
            placeholder: qsTr("Name")
            onAccepted: newCommand.input.forceActiveFocus()
        }
        Field {
            id: newCommand
            width: parent.width
            placeholder: qsTr("Command")
            onAccepted: if (addButton.enabled) addButton.clicked()
        }
        Button {
            id: addButton
            size: "sm"
            variant: "tonal"
            iconPath: Icons.plus
            text: qsTr("Add")
            busy: area.scripts.saving === true
            enabled: newName.text.trim().length > 0 && newCommand.text.trim().length > 0
            onClicked: {
                area.added(newName.text, newCommand.text)
                newName.text = ""
                newCommand.text = ""
            }
        }
    }
    Txt {
        visible: area.refusedText.length > 0
        width: parent.width
        text: area.refusedText
        role: "bodySmall"
        color: Theme.warning
        wrapMode: Text.WordWrap
        elide: Text.ElideNone
    }

    // ---- Phones ----
    Item {
        width: parent.width
        height: Math.max(phonesTexts.implicitHeight + 12, 40)
        Column {
            id: phonesTexts
            anchors.left: parent.left
            anchors.right: phonesToggle.left
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Txt {
                width: parent.width
                text: qsTr("Phones can run these")
                role: "bodySmall"
                weight: 500
            }
            Txt {
                width: parent.width
                text: qsTr("A paired phone with agents on can start and stop this folder's scripts. It asks on the phone each time and tells you here.")
                role: "caption"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
        }
        Toggle {
            id: phonesToggle
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            label: qsTr("Phones can run these")
            checked: area.scripts.phones === true
            onToggled: (on) => Agents.setPhoneScripts(area.folderId, on)
        }
    }
}
