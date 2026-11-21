// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// One step the agent took, as one line: "Ran cargo test", "Edited
// src/hold.rs +4 −2", "Wants to run …". It opens to its command, change
// and output. Its glyph takes its class's tint, its target is coloured as
// the render crate tokenized it (a command as shell, a path as folder and
// name), and it ends with its status as a glyph: a ring while it runs, a
// check on a command that worked, an x when it failed (a muted one when
// denied), a question mark while it asks, a square when stopped.
Item {
    id: tool
    required property string key
    required property int depth
    required property string label
    // The target as markup the app built from escaped text.
    required property string styled
    required property string status
    required property bool hasDetail
    required property bool hasOutput
    required property bool isOpen
    required property int adds
    required property int dels
    required property real ms

    readonly property var parsed: {
        try {
            return JSON.parse(label)
        } catch (e) {
            return ({ class: "other", object: "", more: 0 })
        }
    }
    readonly property string cls: parsed.class || "other"
    readonly property int more: parsed.more || 0
    readonly property bool openable: hasDetail || hasOutput
    readonly property bool failed: status === "failed"
    // Tabler's help-circle (MIT, as Icons.qml credits).
    readonly property string questionMark: "M3 12a9 9 0 1 0 18 0 9 9 0 1 0-18 0M12 17v.01M12 13.5a1.5 1.5 0 0 1 1-1.5 2.6 2.6 0 1 0-3-4"

    // The sentence for this step, with %1 where its object goes.
    readonly property string sentence: {
        const s = status
        switch (cls) {
        case "command":
            return s === "running" ? qsTr("Running %1") : s === "asking" ? qsTr("Wants to run %1")
                : s === "failed" ? qsTr("Failed: %1") : s === "denied" ? qsTr("Denied: %1")
                : s === "stopped" ? qsTr("Stopped: %1") : qsTr("Ran %1")
        case "edit":
            return s === "running" ? qsTr("Editing %1") : s === "asking" ? qsTr("Wants to edit %1")
                : s === "failed" ? qsTr("Couldn't edit %1") : s === "denied" ? qsTr("Denied: edit %1")
                : s === "stopped" ? qsTr("Stopped: edit %1") : qsTr("Edited %1")
        case "read":
            return s === "running" ? qsTr("Reading %1") : s === "asking" ? qsTr("Wants to read %1")
                : s === "failed" ? qsTr("Couldn't read %1") : s === "stopped" ? qsTr("Stopped: read %1")
                : s === "denied" ? qsTr("Denied: %1") : qsTr("Read %1")
        case "search":
            return s === "running" ? qsTr("Searching %1") : s === "asking" ? qsTr("Wants to use %1")
                : s === "failed" ? qsTr("Search failed: %1") : s === "stopped" ? qsTr("Stopped: search %1")
                : s === "denied" ? qsTr("Denied: %1") : qsTr("Searched %1")
        case "web":
            return s === "running" ? qsTr("Searching the web: %1") : s === "asking" ? qsTr("Wants to use %1")
                : s === "failed" ? qsTr("Web search failed: %1") : s === "denied" ? qsTr("Denied: %1")
                : qsTr("Searched the web: %1")
        default:
            return s === "asking" ? qsTr("Wants to use %1") : s === "denied" ? qsTr("Denied: %1") : "%1"
        }
    }
    // The words around the object, in the translation's order.
    readonly property var parts: {
        let around = sentence
        if (more > 0)
            around = around.replace("%1", qsTr("%1 and %n more", "", more))
        const at = around.indexOf("%1")
        return at < 0 ? [around, ""] : [around.slice(0, at), around.slice(at + 2)]
    }
    readonly property string icon: {
        switch (cls) {
        case "command": return Icons.terminal
        case "edit": return Icons.edit
        case "read": return Icons.text
        case "search": return Icons.search
        case "web": return Icons.globe
        case "agent": return Icons.sparkle
        case "tasks": return Icons.clipboardList
        default: return Icons.bolt
        }
    }
    // "38s", "3m 02s": how long a command ran, once it ran a second or more.
    readonly property string took: {
        if (cls !== "command" || ms < 1000)
            return ""
        const total = Math.floor(ms / 1000)
        if (total < 60)
            return qsTr("%1s").arg(total)
        const minutes = Math.floor(total / 60)
        if (minutes < 60)
            return qsTr("%1m %2s").arg(minutes).arg(String(total % 60).padStart(2, "0"))
        return qsTr("%1h %2m").arg(Math.floor(minutes / 60)).arg(String(minutes % 60).padStart(2, "0"))
    }

    width: ListView.view ? ListView.view.width : 0
    height: 28

    Accessible.role: openable ? Accessible.Button : Accessible.StaticText
    Accessible.name: parts[0] + parsed.object + parts[1]
    Accessible.onPressAction: if (tool.openable) Agents.toggle(tool.key)

    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: tool.depth * 18
        radius: Theme.radiusSm
        color: Theme.surfaceContentVariant
        opacity: hover.hovered && tool.openable ? 0.06 : 0
    }
    Icon {
        visible: tool.openable
        x: tool.depth * 18 + 2
        anchors.verticalCenter: parent.verticalCenter
        width: 14
        height: 14
        path: tool.isOpen ? Icons.chevronDown : Icons.chevronRight
        color: Theme.surfaceContentVariant
    }
    Icon {
        id: glyph
        x: tool.depth * 18 + 20
        anchors.verticalCenter: parent.verticalCenter
        width: 16
        height: 16
        path: tool.icon
        color: tool.failed ? AgentPalette.toolFailed : AgentPalette.toolColor(tool.cls)
    }
    Row {
        anchors.left: glyph.right
        anchors.leftMargin: 8
        anchors.right: trailing.left
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        clip: true
        Txt {
            id: before
            anchors.verticalCenter: parent.verticalCenter
            text: tool.parts[0]
            role: "bodySmall"
            muted: true
            elide: Text.ElideNone
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, parent.width - before.width - after.width)
            text: tool.styled.length > 0 ? tool.styled : (tool.parsed.object || "")
            // Markup built by the app from escaped text; never the agent's.
            textFormat: tool.styled.length > 0 ? Text.StyledText : Text.PlainText
            color: tool.status === "running" ? Theme.surfaceContent : Theme.surfaceContentVariant
            font.family: Theme.fontMono
            font.features: Theme.monoFeatures
            font.pixelSize: Theme.typography.bodySmall.size
            elide: Text.ElideRight
            maximumLineCount: 1
        }
        Txt {
            id: after
            anchors.verticalCenter: parent.verticalCenter
            text: tool.parts[1]
            role: "bodySmall"
            muted: true
            elide: Text.ElideNone
        }
    }
    Row {
        id: trailing
        anchors.right: parent.right
        anchors.rightMargin: 4
        anchors.verticalCenter: parent.verticalCenter
        spacing: 6
        Spinner {
            visible: tool.status === "running"
            anchors.verticalCenter: parent.verticalCenter
            width: 13
            height: 13
            color: AgentPalette.working
        }
        Txt {
            visible: tool.took.length > 0
            anchors.verticalCenter: parent.verticalCenter
            text: tool.took
            role: "caption"
            muted: true
        }
        Txt {
            visible: tool.cls === "edit" && (tool.adds > 0 || tool.dels > 0)
            anchors.verticalCenter: parent.verticalCenter
            text: "+" + tool.adds
            role: "caption"
            color: AgentPalette.inserted
        }
        Txt {
            visible: tool.cls === "edit" && (tool.adds > 0 || tool.dels > 0)
            anchors.verticalCenter: parent.verticalCenter
            text: "−" + tool.dels
            role: "caption"
            color: AgentPalette.deleted
        }
        // The status as a glyph; the ok check on commands only, so forty reads aren't forty checks.
        Icon {
            readonly property string shape: {
                switch (tool.status) {
                case "ok": return tool.cls === "command" ? Icons.check : ""
                case "failed":
                case "denied": return Icons.close
                case "asking": return tool.questionMark
                case "stopped": return Icons.stopSquare
                default: return ""
                }
            }
            visible: shape.length > 0
            anchors.verticalCenter: parent.verticalCenter
            width: 14
            height: 14
            path: shape
            color: tool.status === "ok" ? AgentPalette.toolOk
                : tool.status === "failed" ? AgentPalette.toolFailed
                : tool.status === "asking" ? AgentPalette.toolAsking : Theme.surfaceContentVariant
        }
    }
    HoverHandler {
        id: hover
    }
    MouseArea {
        anchors.fill: parent
        enabled: tool.openable
        cursorShape: Qt.PointingHandCursor
        onClicked: Agents.toggle(tool.key)
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: tool.depth > 0
        x: (tool.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
