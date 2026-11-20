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

    // A link in it was clicked: `index` is its number in the markup.
    signal linkActivated(string key, int index)

    readonly property bool heading: blockKind === "heading"
    readonly property bool listItem: blockKind === "item"
    readonly property bool alert: ["note", "tip", "important", "warning", "caution"].indexOf(blockKind) >= 0
    readonly property real indent: depth * 18 + quote * 14 + (listItem ? level * 18 + 20 : 0)
    readonly property color alertColor: {
        switch (blockKind) {
        case "tip": return Theme.tintTeal
        case "important": return Theme.tintLavender
        case "warning": return Theme.tintAmber
        case "caution": return Theme.error
        default: return Theme.tintBlue
        }
    }
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
    readonly property bool footed: last && phase !== "interim" && !streaming

    width: ListView.view ? ListView.view.width : 0
    height: body.y + body.height + (footer.visible ? footer.height : 0) + (cut.visible ? cut.height : 0)
        + (heading ? 6 : 4)

    Accessible.role: Accessible.Paragraph
    Accessible.name: text.text

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
        width: parent.width - x
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
                horizontalAlignment: Text.AlignRight
                muted: true
            }
            Rectangle {
                visible: block.marker === "todo" || block.marker === "done"
                x: -19
                y: 4
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
                    color: Theme.tintGreen
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
                font.pixelSize: block.heading
                    ? (block.level <= 1 ? Theme.typography.title.size + 2
                       : block.level === 2 ? Theme.typography.title.size : Theme.typography.body.size)
                    : Theme.typography.body.size
                font.weight: block.heading ? 650 : Theme.typography.body.weight
                lineHeight: 1.25
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
        height: visible ? 22 : 0
        spacing: 10
        opacity: hover.hovered ? 1 : 0
        Behavior on opacity { enabled: !Theme.reduceMotion; NumberAnimation { duration: Theme.fadeFast } }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            text: block.whenMs > 0 ? new Date(block.whenMs).toLocaleTimeString(Qt.locale(), Locale.ShortFormat) : ""
            role: "caption"
            muted: true
        }
        TextAction {
            id: copy
            property bool done: false
            anchors.verticalCenter: parent.verticalCenter
            text: done ? qsTr("Copied") : qsTr("Copy")
            iconPath: done ? Icons.check : Icons.copy
            iconColor: Theme.surfaceContentVariant
            onClicked: {
                if (Agents.copyText("i:" + block.itemId)) {
                    copy.done = true
                    copiedTimer.restart()
                }
            }
            Timer {
                id: copiedTimer
                interval: 1500
                onTriggered: copy.done = false
            }
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
