// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A change, line by line: removed lines on red, added on green, the words
// that changed in bold, line numbers only when the change has them. Long
// lines scroll sideways. `diff` is the JSON the app built (each line's text
// as markup from escaped text).
Item {
    id: diffView
    property string diff: ""
    // Show each file's name above its lines.
    property bool showPaths: true

    readonly property var doc: {
        try {
            return JSON.parse(diff || "{}")
        } catch (e) {
            return ({ files: [] })
        }
    }
    readonly property var files: doc.files || []

    implicitHeight: column.height

    Column {
        id: column
        width: diffView.width
        spacing: 6
        Repeater {
            model: diffView.files
            delegate: Column {
                id: file
                required property var modelData
                readonly property bool numbered: modelData.numbered === true
                // Gutter width: wide enough for the largest number.
                readonly property int digits: {
                    let most = 0
                    for (const row of modelData.rows || [])
                        most = Math.max(most, row.o || 0, row.n || 0)
                    return String(most).length
                }
                width: column.width
                Item {
                    visible: diffView.showPaths && (file.modelData.path || "").length > 0
                    width: parent.width
                    height: visible ? 24 : 0
                    Txt {
                        anchors.left: parent.left
                        anchors.leftMargin: 8
                        anchors.right: fileCounts.left
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        text: file.modelData.path || ""
                        role: "code"
                        size: Theme.typography.bodySmall.size
                        muted: true
                        elide: Text.ElideLeft
                    }
                    Row {
                        id: fileCounts
                        anchors.right: parent.right
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Txt {
                            text: "+" + (file.modelData.adds || 0)
                            role: "caption"
                            color: Theme.tintGreen
                        }
                        Txt {
                            text: "−" + (file.modelData.dels || 0)
                            role: "caption"
                            color: Theme.error
                        }
                    }
                }
                Flickable {
                    id: lines
                    width: parent.width
                    height: rowsColumn.height
                    contentWidth: Math.max(width, rowsColumn.widest)
                    contentHeight: rowsColumn.height
                    flickableDirection: Flickable.HorizontalFlick
                    boundsBehavior: Flickable.StopAtBounds
                    interactive: contentWidth > width
                    clip: true
                    G7ScrollBar {
                        flickable: lines
                        // On the view itself, not in its moving content.
                        parent: lines
                        orientation: Qt.Horizontal
                        anchors.left: lines.left
                        anchors.right: lines.right
                        anchors.bottom: lines.bottom
                    }
                    Column {
                        id: rowsColumn
                        property real widest: 0
                        width: lines.contentWidth
                        Repeater {
                            model: file.modelData.rows || []
                            delegate: Rectangle {
                                id: line
                                required property var modelData
                                readonly property string kind: modelData.k
                                readonly property real gutter: file.numbered ? (file.digits * 8 + 10) * 2 : 0
                                width: rowsColumn.width
                                height: lineText.height + 2
                                color: kind === "add" ? Theme.wash(Theme.tintGreen)
                                    : kind === "del" ? Theme.wash(Theme.error) : "transparent"
                                Row {
                                    visible: file.numbered && line.kind !== "hunk"
                                    x: 4
                                    anchors.verticalCenter: parent.verticalCenter
                                    Txt {
                                        width: file.digits * 8 + 10
                                        text: (line.modelData.o || 0) > 0 ? String(line.modelData.o) : ""
                                        role: "code"
                                        size: Theme.typography.caption.size
                                        muted: true
                                        horizontalAlignment: Text.AlignRight
                                    }
                                    Txt {
                                        width: file.digits * 8 + 10
                                        text: (line.modelData.n || 0) > 0 ? String(line.modelData.n) : ""
                                        role: "code"
                                        size: Theme.typography.caption.size
                                        muted: true
                                        horizontalAlignment: Text.AlignRight
                                    }
                                }
                                Txt {
                                    x: line.gutter + 6
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: line.kind === "add" ? "+" : (line.kind === "del" ? "−" : "")
                                    role: "code"
                                    color: line.kind === "add" ? Theme.tintGreen : Theme.error
                                }
                                Text {
                                    id: lineText
                                    x: line.gutter + 20
                                    text: line.modelData.s
                                    // Markup built by the app from escaped text; never the agent's.
                                    textFormat: Text.StyledText
                                    wrapMode: Text.NoWrap
                                    color: line.kind === "hunk" ? Theme.surfaceContentVariant : Theme.surfaceContent
                                    font.family: Theme.fontMono
                                    font.features: Theme.monoFeatures
                                    font.pixelSize: Theme.typography.code.size
                                    Component.onCompleted: rowsColumn.widest = Math.max(rowsColumn.widest,
                                                                                         x + implicitWidth + 12)
                                }
                            }
                        }
                    }
                }
            }
        }
        Txt {
            visible: diffView.doc.more === true
            x: 8
            text: qsTr("The rest of the change isn't shown.")
            role: "caption"
            muted: true
        }
    }
}
