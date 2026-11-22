// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The run panel (§X5.1), in the slot the source-control panel uses: the
// thread's runs (running first; "from Pixel" for a phone's), then the
// selected run's output, plain and in mono, following the bottom until you
// scroll up; then its state, Stop, Run again, Copy output and Add to
// message. Nothing here is read as markup: the host cleaned each line.
Item {
    id: panel
    // Agents.runs, parsed.
    property var runs: ({})
    property real now: Date.now()
    signal closed

    readonly property var list: runs.runs || []
    readonly property string selectedId: runs.selected || ""
    readonly property var selected: list.find((r) => r.id === panel.selectedId) || ({})
    readonly property var output: runs.output || ({})
    readonly property var lines: output.lines || []
    readonly property bool running: selected.phase === "running"
    readonly property var error: runs.error || ({})

    // "1:05", "1:02:03".
    function duration(ms) {
        const total = Math.max(0, Math.floor(ms / 1000))
        const h = Math.floor(total / 3600)
        const m = Math.floor((total % 3600) / 60)
        const s = String(total % 60).padStart(2, "0")
        return h > 0 ? h + ":" + String(m).padStart(2, "0") + ":" + s : m + ":" + s
    }
    function stateText(r) {
        switch (r.phase) {
        case "running": return qsTr("Running · %1").arg(duration(panel.now - (r.startedMs || 0)))
        case "done": return qsTr("Finished · %1").arg(duration((r.endedMs || 0) - (r.startedMs || 0)))
        case "stopped": return qsTr("Stopped")
        case "failed":
            if (r.reason === "spawn")
                return qsTr("Couldn't start it.")
            return r.exit !== null && r.exit !== undefined ? qsTr("Failed · exit %1").arg(r.exit) : qsTr("Failed")
        default: return ""
        }
    }
    readonly property string errorText: {
        switch (error.reason || "") {
        case "": return ""
        case "script_running": return qsTr("“%1” is already running here.").arg(error.name || "")
        case "runs": return qsTr("Too many scripts are running. Stop one first.")
        case "folder_missing": return qsTr("This folder is missing.")
        case "no_script":
        case "not_found": return qsTr("That script isn't in this folder's list any more.")
        case "unreachable": return qsTr("Couldn't reach the agent host")
        case "denied": return qsTr("That is done from G7 Link on this PC.")
        default: return qsTr("Something went wrong. Try again.")
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
    }
    Item {
        id: head
        width: parent.width
        height: 36
        Txt {
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Run")
            role: "label"
            muted: true
        }
        IconButton {
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.close
            iconSize: 14
            label: qsTr("Close")
            onClicked: panel.closed()
        }
    }

    // ---- The thread's runs ----
    Flickable {
        id: runList
        anchors.top: head.bottom
        x: 8
        width: parent.width - 16
        height: Math.min(runColumn.height, 5 * 44 + 8)
        contentHeight: runColumn.height
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        Column {
            id: runColumn
            width: runList.width
            spacing: 2
            Txt {
                visible: panel.list.length === 0 && panel.runs.loading !== true
                x: 6
                width: parent.width - 12
                height: 32
                verticalAlignment: Text.AlignVCenter
                text: qsTr("No runs in this thread yet.")
                role: "bodySmall"
                muted: true
            }
            Repeater {
                model: panel.list
                delegate: Rectangle {
                    id: runRow
                    required property var modelData
                    width: runColumn.width
                    height: 42
                    radius: Theme.radiusMd
                    readonly property bool picked: runRow.modelData.id === panel.selectedId
                    color: runRow.picked ? Theme.activeFill : runHover.hovered ? Theme.selection : "transparent"
                    activeFocusOnTab: true
                    border.width: runRow.activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                    Accessible.role: Accessible.Button
                    Accessible.name: runRow.modelData.name + ", " + panel.stateText(runRow.modelData)
                    Accessible.onPressAction: Agents.runSelect(runRow.modelData.id)
                    Keys.onReturnPressed: Agents.runSelect(runRow.modelData.id)
                    Keys.onSpacePressed: Agents.runSelect(runRow.modelData.id)
                    HoverHandler {
                        id: runHover
                        cursorShape: Qt.PointingHandCursor
                    }
                    TapHandler {
                        onTapped: Agents.runSelect(runRow.modelData.id)
                    }
                    Spinner {
                        id: runRing
                        visible: runRow.modelData.phase === "running"
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                    }
                    Icon {
                        visible: !runRing.visible
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        path: runRow.modelData.phase === "done" ? AgentGitGlyphs.passed
                            : runRow.modelData.phase === "stopped" ? Icons.stopSquare : AgentGitGlyphs.failed
                        color: runRow.modelData.phase === "done" ? AgentPalette.done
                            : runRow.modelData.phase === "stopped" ? Theme.surfaceContentVariant : AgentPalette.failed
                    }
                    Column {
                        x: 32
                        anchors.verticalCenter: parent.verticalCenter
                        width: parent.width - x - 10
                        spacing: 1
                        Txt {
                            width: parent.width
                            text: runRow.modelData.name
                            role: "bodySmall"
                            weight: 500
                            maximumLineCount: 1
                        }
                        Txt {
                            width: parent.width
                            text: [panel.stateText(runRow.modelData),
                                   (runRow.modelData.by || "").length > 0 ? qsTr("from %1").arg(runRow.modelData.by) : ""]
                                .filter((x) => x.length > 0).join(" · ")
                            role: "caption"
                            muted: true
                            maximumLineCount: 1
                        }
                    }
                }
            }
        }
    }
    Rectangle {
        id: runsLine
        anchors.top: runList.bottom
        anchors.topMargin: 4
        width: parent.width
        height: 1
        color: Theme.line
    }

    // ---- The selected run's output ----
    Rectangle {
        id: outputBox
        anchors.top: runsLine.bottom
        anchors.topMargin: 8
        anchors.bottom: foot.top
        anchors.bottomMargin: 8
        x: 10
        width: parent.width - 20
        radius: Theme.radiusSm
        color: AgentPalette.codeGround
        visible: panel.selectedId.length > 0
        ListView {
            id: outputList
            // It follows new lines until the user scrolls up.
            property bool follow: true
            anchors.fill: parent
            anchors.margins: 8
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: panel.lines
            onCountChanged: if (follow) Qt.callLater(() => outputList.positionViewAtEnd())
            onMovementEnded: follow = atYEnd
            header: Txt {
                visible: (panel.output.dropped || 0) > 0
                height: visible ? 17 : 0
                text: qsTr("%n earlier lines not kept", "", panel.output.dropped || 0)
                role: "caption"
                muted: true
            }
            delegate: Text {
                required property string modelData
                width: ListView.view.width
                text: modelData
                textFormat: Text.PlainText
                wrapMode: Text.NoWrap
                elide: Text.ElideRight
                color: AgentPalette.diffText
                font.family: Theme.fontMono
                font.features: Theme.monoFeatures
                font.pixelSize: Theme.typography.caption.size
                height: 17
                verticalAlignment: Text.AlignVCenter
            }
            TouchpadGlide { view: outputList }
        }
        G7ScrollBar {
            flickable: outputList
            anchors.top: outputList.top
            anchors.bottom: outputList.bottom
            anchors.right: outputList.right
        }
    }
    Connections {
        target: panel
        function onSelectedIdChanged() { outputList.follow = true }
    }

    // ---- Its state and what can be done with it ----
    Column {
        id: foot
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 10
        x: 12
        width: parent.width - 24
        spacing: 8
        Txt {
            visible: panel.errorText.length > 0
            width: parent.width
            text: panel.errorText
            role: "bodySmall"
            color: Theme.warning
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Txt {
            visible: panel.selectedId.length > 0
            width: parent.width
            text: panel.stateText(panel.selected)
            role: "bodySmall"
            color: panel.selected.phase === "failed" ? AgentPalette.failedText : Theme.surfaceContent
            maximumLineCount: 1
        }
        Flow {
            visible: panel.selectedId.length > 0
            width: parent.width
            spacing: 6
            Button {
                visible: panel.running
                size: "sm"
                variant: "tonal"
                iconPath: Icons.stopSquare
                text: qsTr("Stop")
                onClicked: Agents.scriptStop(panel.selectedId)
            }
            Button {
                visible: !panel.running && panel.runs.available === true
                size: "sm"
                variant: "tonal"
                iconPath: Icons.play
                text: qsTr("Run again")
                onClicked: Agents.scriptRun(panel.selected.scriptId || "")
            }
            Button {
                size: "sm"
                variant: "text"
                iconPath: Icons.copy
                text: qsTr("Copy output")
                enabled: panel.lines.length > 0
                onClicked: Agents.runCopy(panel.selectedId)
            }
            Button {
                visible: panel.runs.available === true
                size: "sm"
                variant: "text"
                iconPath: Icons.plus
                text: qsTr("Add to message")
                enabled: panel.lines.length > 0
                onClicked: Agents.runQuote(panel.selectedId)
            }
        }
    }
}
