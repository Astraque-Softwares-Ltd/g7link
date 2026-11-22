// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A pull request's description or a comment (§X6.6): its Markdown blocks as
// markup the app built, everything escaped (raw HTML stays text, no image
// loads). A link reports its key and number; the page asks before it opens.
Column {
    id: comment
    // [{ key, kind, level, depth, quote, marker, number, markup }]
    property var blocks: []
    signal linkActivated(string key, int index)

    spacing: 6

    Repeater {
        model: comment.blocks
        delegate: Item {
            id: block
            required property var modelData
            readonly property bool code: modelData.kind === "code" || modelData.kind === "table"
            readonly property bool heading: modelData.kind === "heading"
            readonly property bool listItem: modelData.kind === "item"
            readonly property real indent: (modelData.depth || 0) * 18 + (modelData.quote || 0) * 14
                + (listItem ? 18 : 0)
            readonly property string markerText: modelData.marker === "bullet" ? "•"
                : modelData.marker === "number" ? (modelData.number || 0) + "."
                : modelData.marker === "done" ? "☑" : modelData.marker === "todo" ? "☐" : ""
            width: comment.width
            height: modelData.kind === "rule" ? 9 : code ? codeBox.height : textLine.height

            Rectangle {
                visible: block.modelData.kind === "rule"
                y: 4
                width: parent.width
                height: 1
                color: Theme.line
            }
            Rectangle {
                visible: (block.modelData.quote || 0) > 0
                x: block.indent - 10
                width: 2
                height: parent.height
                color: Theme.outlineVariant
            }
            Txt {
                visible: block.listItem && block.markerText.length > 0
                x: block.indent - 16
                text: block.markerText
                role: "bodySmall"
                muted: true
            }
            Text {
                id: textLine
                visible: !block.code && block.modelData.kind !== "rule"
                x: block.indent
                width: parent.width - x
                text: block.modelData.markup || ""
                textFormat: Text.StyledText
                wrapMode: Text.Wrap
                color: Theme.surfaceContent
                linkColor: Theme.accent
                font.family: Theme.fontUi
                font.pixelSize: block.heading ? Theme.typography.body.size + 2 : Theme.typography.bodySmall.size
                font.weight: block.heading ? 650 : Theme.typography.bodySmall.weight
                lineHeight: 1.25
                onLinkActivated: (link) => comment.linkActivated(block.modelData.key, parseInt(link))
                HoverHandler {
                    cursorShape: textLine.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor
                }
            }
            Rectangle {
                id: codeBox
                visible: block.code
                x: block.indent
                width: parent.width - x
                height: visible ? codeText.implicitHeight + 16 : 0
                radius: Theme.radiusSm
                color: AgentPalette.codeGround
                clip: true
                Text {
                    id: codeText
                    x: 8
                    y: 8
                    width: parent.width - 16
                    text: block.modelData.markup || ""
                    textFormat: Text.StyledText
                    wrapMode: Text.WrapAnywhere
                    color: AgentPalette.diffText
                    font.family: Theme.fontMono
                    font.features: Theme.monoFeatures
                    font.pixelSize: Theme.typography.caption.size
                }
            }
        }
    }
}
