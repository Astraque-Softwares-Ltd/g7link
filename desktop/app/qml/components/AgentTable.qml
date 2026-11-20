// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A table the agent wrote: header row, cells (markup the app built from
// escaped text), sideways scroll when it's wide, and how many rows were
// left out.
Item {
    id: sheet
    required property string key
    required property int depth
    required property int quote
    required property string table

    signal linkActivated(string key, int index)

    readonly property var parsed: {
        try {
            return JSON.parse(sheet.table)
        } catch (e) {
            return ({ columns: 0, align: [], more: 0, cells: [] })
        }
    }
    readonly property int columns: Math.max(1, parsed.columns || 0)

    width: ListView.view ? ListView.view.width : 0
    height: grid.height + (moreText.visible ? moreText.height + 6 : 0) + 12

    Flickable {
        id: scroll
        x: sheet.depth * 18 + sheet.quote * 14
        y: 6
        width: parent.width - x
        height: grid.height
        contentWidth: Math.max(width, grid.width)
        contentHeight: grid.height
        flickableDirection: Flickable.HorizontalFlick
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentWidth > width
        clip: true
        Grid {
            id: grid
            columns: sheet.columns
            columnSpacing: 0
            rowSpacing: 0
            Repeater {
                model: sheet.parsed.cells || []
                delegate: Item {
                    id: cell
                    required property var modelData
                    required property int index
                    readonly property string align: (sheet.parsed.align || [])[index % sheet.columns] || ""
                    width: Math.min(cellText.implicitWidth, 360) + 20
                    height: Math.max(28, cellText.height + 10)
                    Text {
                        id: cellText
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(implicitWidth, 360)
                        text: cell.modelData.styled
                        // Markup built by the app from escaped text; never the agent's.
                        textFormat: Text.StyledText
                        wrapMode: Text.Wrap
                        horizontalAlignment: cell.align === "right" ? Text.AlignRight
                            : (cell.align === "center" ? Text.AlignHCenter : Text.AlignLeft)
                        color: cell.modelData.header ? Theme.surfaceContentVariant : Theme.surfaceContent
                        linkColor: Theme.accent
                        font.family: Theme.fontUi
                        font.pixelSize: cell.modelData.header ? Theme.typography.bodySmall.size
                                                              : Theme.typography.body.size
                        onLinkActivated: (link) => sheet.linkActivated(sheet.key, parseInt(link))
                    }
                    Rectangle {
                        anchors.bottom: parent.bottom
                        width: parent.width
                        height: 1
                        color: Theme.line
                    }
                }
            }
        }
    }
    G7ScrollBar {
        flickable: scroll
        orientation: Qt.Horizontal
        anchors.left: scroll.left
        anchors.right: scroll.right
        anchors.bottom: scroll.bottom
        anchors.bottomMargin: -6
    }
    Rectangle {
        id: moreText
        visible: (sheet.parsed.more || 0) > 0
        x: scroll.x
        anchors.top: scroll.bottom
        anchors.topMargin: 6
        width: moreLabel.implicitWidth + 16
        height: visible ? 24 : 0
        radius: Theme.radiusSm
        color: "transparent"
        border.width: 1
        border.color: Theme.outlineVariant
        Txt {
            id: moreLabel
            anchors.centerIn: parent
            text: qsTr("%n more row(s)", "", sheet.parsed.more || 0)
            role: "caption"
        }
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: sheet.depth > 0
        x: (sheet.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
