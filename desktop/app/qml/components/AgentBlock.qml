// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// One block of what the agent wrote: a paragraph, a heading, a list item, a
// rule or a note. Its text is markup the app built from the agent's text,
// everything escaped (`styled`): never the agent's own markup. Under the
// last block of an answer, hovering shows its time and Copy.
Item {
    id: block
    required property string key
    required property string itemId
    required property int depth
    required property string styled
    required property string blockKind
    required property int level
    required property int quote
    required property string marker
    required property int markerNumber
    required property bool last
    required property string phase
    required property bool streaming
    required property real whenMs
    required property bool truncated
    // "body" inside a plan's card.
    required property string planPart
    required property int turn
    // "Fork from here" on the final answer's last block (see AgentRowActions).
    required property string actFork
    // For the reason an action can't be done.
    property string agentName
    // The message a search hit opened the thread at, lit up for a moment.
    property string highlightItem: ""
    readonly property bool lit: highlightItem.length > 0 && highlightItem === itemId

    // A link in it was clicked: `index` is its number in the markup.
    signal linkActivated(string key, int index)
    // "Fork from here" on the answer: a new thread through this turn.
    signal forkRequested(int turn)

    readonly property bool heading: blockKind === "heading"
    readonly property bool listItem: blockKind === "item"
    readonly property bool alert: ["note", "tip", "important", "warning", "caution"].indexOf(blockKind) >= 0
    readonly property bool inPlan: planPart === "body"
    // Inside a plan's card, its text keeps clear of the card's sides.
    readonly property real planInset: inPlan ? 16 : 0
    readonly property real indent: depth * 18 + quote * 14 + (listItem ? level * 18 + 20 : 0) + planInset
    readonly property color alertColor: AgentPalette.alert(blockKind)
    readonly property string alertWord: {
        switch (blockKind) {
        case "tip": return qsTr("Tip")
        case "important": return qsTr("Important")
        case "warning": return qsTr("Warning")
        case "caution": return qsTr("Caution")
        default: return qsTr("Note")
        }
    }
    readonly property string markerText: {
        switch (marker) {
        case "bullet": return level % 2 === 0 ? "•" : "◦"
        case "number": return markerNumber + "."
        default: return ""
        }
    }
    // The answer's own footer: its time and Copy, on hover.
    readonly property bool footed: last && phase !== "interim" && !streaming && !inPlan

    width: ListView.view ? ListView.view.width : 0
    height: body.y + body.height + (footer.visible ? footer.height : 0) + (cut.visible ? cut.height : 0)
        + (heading ? 6 : 4)

    Accessible.role: Accessible.Paragraph
    Accessible.name: text.text

    Rectangle {
        visible: block.lit
        x: block.indent - 10
        width: parent.width - x
        height: parent.height
        radius: Theme.radiusMd
        color: Theme.selection
        border.width: 1.5
        border.color: Theme.accent
    }

    AgentPlanFrame {
        visible: block.inPlan
        x: block.depth * 18
        width: parent.width - x
        height: parent.height
    }

    // Block-quote bars, one per level.
    Repeater {
        model: block.quote
        delegate: Rectangle {
            required property int index
            x: block.depth * 18 + index * 14 + 2
            y: 2
            width: 2
            height: body.y + body.height - 2
            color: Theme.outlineVariant
        }
    }

    Column {
        id: body
        x: block.indent
        y: block.heading ? 8 : 3
        width: parent.width - x - block.planInset
        spacing: 2

        Txt {
            visible: block.alert
            text: block.alertWord
            role: "label"
            color: block.alertColor
        }
        Rectangle {
            visible: block.blockKind === "rule"
            width: parent.width
            height: visible ? 13 : 0
            color: "transparent"
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 1
                color: Theme.line
            }
        }
        Item {
            visible: block.blockKind !== "rule"
            width: parent.width
            height: text.height
            Txt {
                visible: block.listItem && block.markerText.length > 0
                x: -18
                width: 16
                text: block.markerText
                role: "prose"
                horizontalAlignment: Text.AlignRight
                muted: true
            }
            Rectangle {
                visible: block.marker === "todo" || block.marker === "done"
                x: -19
                y: Math.round((text.lineHeight - height) / 2)
                width: 13
                height: 13
                radius: 3
                color: "transparent"
                border.width: 1.2
                border.color: Theme.outline
                Icon {
                    visible: block.marker === "done"
                    anchors.centerIn: parent
                    width: 11
                    height: 11
                    path: Icons.check
                    color: AgentPalette.doneBox
                }
            }
            Text {
                id: text
                objectName: "text"
                width: parent.width
                text: block.styled
                // Markup built by the app from escaped text; never the agent's.
                textFormat: Text.StyledText
                wrapMode: Text.Wrap
                color: block.quote > 0 ? Theme.surfaceContentVariant : Theme.surfaceContent
                linkColor: Theme.accent
                font.family: Theme.fontUi
                // The answer reads as prose, a size up from the app's text;
                // headings stand above it.
                font.pixelSize: block.heading && block.level <= 1 ? Theme.typography.prose.size + 2
                    : Theme.typography.prose.size
                font.weight: block.heading ? 650 : Theme.typography.prose.weight
                lineHeightMode: Text.FixedHeight
                lineHeight: Math.round(font.pixelSize * (block.heading ? 1.3 : Theme.typography.prose.lineHeight))
                onLinkActivated: (link) => block.linkActivated(block.key, parseInt(link))
                HoverHandler {
                    cursorShape: text.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor
                }
            }
            Rectangle {
                visible: block.alert
                x: -10
                width: 2
                height: parent.height
                color: block.alertColor
            }
            // Level 1 and 2 headings: a lavender rule, the text in ink.
            Rectangle {
                visible: block.heading && block.level <= 2 && !block.inPlan
                x: -10
                width: 2
                height: parent.height
                color: AgentPalette.headingRule
            }
        }
    }

    Txt {
        id: cut
        visible: block.last && block.truncated
        anchors.top: body.bottom
        anchors.topMargin: 4
        x: block.indent
        height: visible ? implicitHeight : 0
        text: qsTr("This message was cut short.")
        role: "caption"
        muted: true
    }

    Row {
        id: footer
        visible: block.footed
        anchors.top: cut.visible ? cut.bottom : body.bottom
        anchors.topMargin: 2
        x: block.indent
        height: visible ? 26 : 0
        spacing: 10
        opacity: hover.hovered ? 1 : 0
        Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            text: block.whenMs > 0 ? new Date(block.whenMs).toLocaleTimeString(Qt.locale(), Locale.ShortFormat) : ""
            role: "caption"
            muted: true
        }
        AgentRowActions {
            anchors.verticalCenter: parent.verticalCenter
            itemId: block.itemId
            fork: block.actFork
            agentName: block.agentName
            onForkRequested: block.forkRequested(block.turn)
        }
    }
    HoverHandler {
        id: hover
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: block.depth > 0
        x: (block.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
