// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// "Bring in earlier sessions": every Claude and Codex session on this PC,
// by folder, newest first; an agent filter and a search over titles and
// folders. Pick several and bring them in; each becomes a thread you can
// read at once, and sending to one continues a copy (the original session
// stays as it is). A row opens a preview first.
Item {
    id: screen
    // Agents.import, parsed.
    property var importState: ({})
    property bool busy: false

    signal previewed(string id)

    readonly property var groups: importState.groups || []
    readonly property var job: importState.job || ({})
    readonly property bool importing: job.total !== undefined
    readonly property var done: importState.done || ({})
    readonly property int selected: importState.selected || 0
    readonly property bool empty: groups.length === 0 && importState.scanning !== true && importState.loading !== true
        && (importState.query || "").length === 0 && (importState.agent || "").length === 0

    // The search box takes the keyboard when the screen opens.
    function focusSearch() {
        searchField.input.forceActiveFocus()
    }

    Row {
        id: controls
        x: 16
        y: 10
        width: parent.width - 32
        spacing: 12
        visible: !screen.empty
        Segmented {
            id: agentFilter
            anchors.verticalCenter: parent.verticalCenter
            options: [
                { value: "", label: qsTr("All") },
                { value: "claude", label: "Claude" },
                { value: "codex", label: "Codex" }
            ]
            value: screen.importState.agent || ""
            onPicked: (value) => Agents.importAgent(value.length > 0 ? value : "all")
        }
        Field {
            id: searchField
            width: parent.width - agentFilter.width - 12
            placeholder: qsTr("Search sessions")
            iconPath: Icons.search
            onTextChanged: Agents.importQuery(text)
        }
    }

    Flickable {
        id: list
        anchors.top: controls.visible ? controls.bottom : parent.top
        anchors.topMargin: 8
        anchors.bottom: bar.top
        x: 8
        width: parent.width - 16
        contentHeight: listColumn.height
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        Column {
            id: listColumn
            width: list.width
            bottomPadding: 8
            spacing: 6

            // The last import: how many came in, and those that couldn't be read.
            Rectangle {
                visible: screen.done.brought !== undefined
                width: parent.width
                height: visible ? doneColumn.height + 12 : 0
                radius: Theme.radiusMd
                color: "transparent"
                border.width: 1
                border.color: Theme.outlineVariant
                Column {
                    id: doneColumn
                    x: 6
                    y: 6
                    width: parent.width - 12
                    Item {
                        width: parent.width
                        height: 28
                        Row {
                            x: 6
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 8
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 15
                                height: 15
                                path: (screen.done.failed || []).length > 0 ? Icons.warning : Icons.check
                                color: (screen.done.failed || []).length > 0 ? AgentPalette.needsYou : AgentPalette.done
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: (screen.done.failed || []).length > 0
                                    ? qsTr("%1 brought in, %2 couldn't be read").arg(screen.done.brought || 0)
                                      .arg((screen.done.failed || []).length)
                                    : qsTr("%n brought in", "", screen.done.brought || 0)
                                weight: 500
                            }
                        }
                    }
                    Repeater {
                        model: screen.done.failed || []
                        delegate: Item {
                            id: failedRow
                            required property var modelData
                            width: doneColumn.width
                            height: 28
                            Txt {
                                x: 29
                                anchors.right: failedPath.left
                                anchors.rightMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                text: failedRow.modelData.title
                                maximumLineCount: 1
                            }
                            Txt {
                                id: failedPath
                                anchors.right: failedAgent.left
                                anchors.rightMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.min(implicitWidth, 200)
                                text: failedRow.modelData.path
                                role: "code"
                                size: Theme.typography.caption.size
                                muted: true
                                elide: Text.ElideLeft
                            }
                            Txt {
                                id: failedAgent
                                anchors.right: parent.right
                                anchors.rightMargin: 6
                                anchors.verticalCenter: parent.verticalCenter
                                text: failedRow.modelData.agentName
                                role: "bodySmall"
                                muted: true
                            }
                        }
                    }
                }
            }

            // Still looking: what's found so far shows meanwhile.
            Item {
                visible: screen.importState.scanning === true
                width: parent.width
                height: visible ? 30 : 0
                Row {
                    x: Theme.rowPadding
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8
                    Spinner {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                    }
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Looking for sessions… %1 so far").arg((screen.importState.total || 0).toLocaleString(Qt.locale(), "f", 0))
                        role: "bodySmall"
                        muted: true
                    }
                }
            }

            Repeater {
                model: screen.groups
                delegate: Column {
                    id: group
                    required property var modelData
                    width: listColumn.width
                    Item {
                        width: parent.width
                        height: 30
                        Rectangle {
                            id: groupBox
                            x: Theme.rowPadding
                            anchors.verticalCenter: parent.verticalCenter
                            width: 14
                            height: 14
                            radius: 3
                            opacity: group.modelData.selectable && !screen.importing ? 1 : 0.4
                            color: group.modelData.selected ? Theme.accent : "transparent"
                            border.width: group.modelData.selected ? 0 : 1.2
                            border.color: Theme.outline
                            Accessible.role: Accessible.CheckBox
                            Accessible.checked: group.modelData.selected === true
                            Accessible.name: qsTr("Select all in %1").arg(group.modelData.path)
                            Icon {
                                visible: group.modelData.selected === true
                                anchors.centerIn: parent
                                width: 11
                                height: 11
                                path: Icons.check
                                color: Theme.accentContent
                            }
                            MouseArea {
                                anchors.fill: parent
                                anchors.margins: -6
                                enabled: group.modelData.selectable === true && !screen.importing
                                cursorShape: Qt.PointingHandCursor
                                onClicked: Agents.importPickGroup(group.modelData.key)
                            }
                        }
                        Row {
                            anchors.left: groupBox.right
                            anchors.leftMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 8
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                                path: Icons.folder
                                color: AgentPalette.needsYou
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: group.modelData.path
                                role: "code"
                                size: Theme.typography.bodySmall.size
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("%n session(s)", "", group.modelData.count)
                                role: "bodySmall"
                                muted: true
                            }
                        }
                    }
                    Repeater {
                        model: group.modelData.rows
                        delegate: AgentFoundRow {
                            required property var modelData
                            width: group.width
                            item: modelData
                            busy: screen.importing
                            selected: Agents.openKind === "preview" && Agents.openId === modelData.id
                            onPicked: Agents.importPick(modelData.id)
                            onPreviewed: screen.previewed(modelData.id)
                        }
                    }
                }
            }
            TextAction {
                visible: screen.importState.more === true
                x: Theme.rowPadding
                enabled: screen.importState.loading !== true
                text: qsTr("Load earlier")
                iconPath: Icons.history
                onClicked: Agents.importMore()
            }
            Txt {
                visible: screen.groups.length === 0 && !screen.empty && screen.importState.loading !== true
                         && screen.importState.scanning !== true
                x: Theme.rowPadding
                topPadding: 8
                text: qsTr("Nothing matches.")
                role: "bodySmall"
                muted: true
            }
        }
        TouchpadGlide { view: list }
    }
    G7ScrollBar {
        flickable: list
        anchors.top: list.top
        anchors.bottom: list.bottom
        anchors.right: list.right
    }

    Txt {
        visible: screen.empty && screen.importState.failed !== true
        anchors.centerIn: parent
        text: qsTr("No sessions found on this PC.")
        muted: true
    }

    // The picked ones and Bring in; while it runs, how far it got and Cancel.
    Item {
        id: bar
        anchors.bottom: parent.bottom
        width: parent.width
        height: screen.importing || screen.selected > 0 ? 52 : 0
        visible: height > 0
        Rectangle {
            width: parent.width
            height: 1
            color: Theme.line
        }
        Row {
            x: 16
            anchors.verticalCenter: parent.verticalCenter
            spacing: 12
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: screen.importing
                    ? qsTr("Bringing in %1 of %2").arg(Math.min((screen.job.done || 0) + 1, screen.job.total)).arg(screen.job.total)
                    : qsTr("%n selected", "", screen.selected)
                weight: 500
            }
            Rectangle {
                visible: screen.importing
                anchors.verticalCenter: parent.verticalCenter
                width: 220
                height: 4
                radius: 2
                color: Theme.selection
                Accessible.role: Accessible.ProgressBar
                Rectangle {
                    width: parent.width * (screen.job.total > 0 ? Math.min(1, (screen.job.done || 0) / screen.job.total) : 0)
                    height: parent.height
                    radius: 2
                    color: Theme.accent
                }
            }
        }
        Row {
            anchors.right: parent.right
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10
            Button {
                visible: !screen.importing
                variant: "text"
                size: "sm"
                text: qsTr("Clear")
                onClicked: Agents.importClear()
            }
            Button {
                visible: !screen.importing
                iconPath: Icons.download
                text: qsTr("Bring in %n", "", screen.selected)
                busy: screen.busy
                onClicked: Agents.importStart("")
            }
            Button {
                visible: screen.importing
                variant: "tonal"
                text: qsTr("Cancel")
                onClicked: Agents.importCancel()
            }
        }
    }
}
