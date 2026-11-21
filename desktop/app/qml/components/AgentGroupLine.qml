// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Several finished steps as one line, "Ran 3 commands and changed 2 files",
// that opens to them. Its glyph is red when one of them failed.
Item {
    id: group
    required property string key
    required property int depth
    required property string summary
    required property int count
    required property bool failed
    required property bool isOpen

    readonly property var parsed: {
        try {
            return JSON.parse(summary)
        } catch (e) {
            return ({ first: { class: "other", count: count }, second: null, others: 0 })
        }
    }

    // A clause as it starts the line, and as it follows "and".
    function clause(c, lower) {
        const n = c ? c.count : 0
        switch (c ? c.class : "other") {
        case "command": return lower ? qsTr("ran %n command(s)", "", n) : qsTr("Ran %n command(s)", "", n)
        case "edit": return lower ? qsTr("changed %n file(s)", "", n) : qsTr("Changed %n file(s)", "", n)
        case "read": return lower ? qsTr("read %n file(s)", "", n) : qsTr("Read %n file(s)", "", n)
        case "search": return lower ? qsTr("searched %n time(s)", "", n) : qsTr("Searched %n time(s)", "", n)
        case "web": return lower ? qsTr("searched the web %n time(s)", "", n)
                                 : qsTr("Searched the web %n time(s)", "", n)
        default: return lower ? qsTr("used %n tool(s)", "", n) : qsTr("Used %n tool(s)", "", n)
        }
    }
    readonly property string line: {
        const first = clause(parsed.first, false)
        const others = parsed.others || 0
        if (!parsed.second)
            return others > 0 ? qsTr("%1 and %n other action(s)", "", others).arg(first) : first
        const second = clause(parsed.second, true)
        return others > 0 ? qsTr("%1, %2 and %n other action(s)", "", others).arg(first).arg(second)
                          : qsTr("%1 and %2").arg(first).arg(second)
    }
    readonly property string icon: {
        switch (parsed.first ? parsed.first.class : "") {
        case "command": return Icons.terminal
        case "edit": return Icons.edit
        case "read": return Icons.text
        case "search": return Icons.search
        case "web": return Icons.globe
        default: return Icons.bolt
        }
    }

    width: ListView.view ? ListView.view.width : 0
    height: 28

    Accessible.role: Accessible.Button
    Accessible.name: line
    Accessible.onPressAction: Agents.toggle(group.key)

    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: group.depth * 18
        radius: Theme.radiusSm
        color: Theme.surfaceContentVariant
        opacity: hover.hovered ? 0.06 : 0
    }
    Icon {
        x: group.depth * 18 + 2
        anchors.verticalCenter: parent.verticalCenter
        width: 14
        height: 14
        path: group.isOpen ? Icons.chevronDown : Icons.chevronRight
        color: Theme.surfaceContentVariant
    }
    Icon {
        id: glyph
        x: group.depth * 18 + 20
        anchors.verticalCenter: parent.verticalCenter
        width: 16
        height: 16
        path: group.icon
        color: group.failed ? AgentPalette.failed : Theme.surfaceContentVariant
    }
    Txt {
        anchors.left: glyph.right
        anchors.leftMargin: 8
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        text: group.line
        role: "bodySmall"
        muted: true
        maximumLineCount: 1
    }
    HoverHandler {
        id: hover
    }
    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: Agents.toggle(group.key)
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: group.depth > 0
        x: (group.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
