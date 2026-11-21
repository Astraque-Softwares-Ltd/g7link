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
    // One file filling the view, its lines made as they scroll into view
    // (the diff screen); otherwise every line is laid out (under a step).
    property bool paged: false

    readonly property var doc: {
        try {
            return JSON.parse(diff || "{}")
        } catch (e) {
            return ({ files: [] })
        }
    }
    readonly property var files: doc.files || []

    implicitHeight: paged ? 0 : column.height

    FontMetrics {
        id: mono
        font.family: Theme.fontMono
        font.pixelSize: Theme.typography.code.size
    }

    // ---- One file, a line at a time, on the code ground ----
    Rectangle {
        visible: diffView.paged
        anchors.fill: parent
        color: AgentPalette.codeGround
    }
    Flickable {
        id: sideways
        visible: diffView.paged
        anchors.fill: parent
        readonly property var file: diffView.files.length > 0 ? diffView.files[0] : ({ rows: [] })
        readonly property int digits: {
            let most = 0
            for (const row of file.rows || [])
                most = Math.max(most, row.o || 0, row.n || 0)
            return String(most).length
        }
        readonly property real gutter: file.numbered === true ? (digits * 8 + 10) * 2 : 0
        contentWidth: Math.max(width, gutter + 32 + (file.widest || 0) * mono.averageCharacterWidth)
        contentHeight: height
        flickableDirection: Flickable.HorizontalFlick
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentWidth > width
        clip: true
        ListView {
            id: pagedRows
            width: sideways.contentWidth
            height: sideways.height
            model: diffView.paged ? (sideways.file.rows || []) : []
            boundsBehavior: Flickable.StopAtBounds
            delegate: Rectangle {
                id: pagedLine
                required property var modelData
                readonly property string kind: modelData.k
                width: pagedRows.width
                height: pagedText.height + 2
                color: kind === "add" ? AgentPalette.addRow
                    : kind === "del" ? AgentPalette.delRow
                    : kind === "hunk" ? AgentPalette.hunkGround : "transparent"
                Row {
                    visible: sideways.file.numbered === true && pagedLine.kind !== "hunk"
                    x: 4
                    anchors.verticalCenter: parent.verticalCenter
                    Txt {
                        width: sideways.digits * 8 + 10
                        text: (pagedLine.modelData.o || 0) > 0 ? String(pagedLine.modelData.o) : ""
                        role: "code"
                        size: Theme.typography.caption.size
                        color: AgentPalette.lineNo
                        horizontalAlignment: Text.AlignRight
                    }
                    Txt {
                        width: sideways.digits * 8 + 10
                        text: (pagedLine.modelData.n || 0) > 0 ? String(pagedLine.modelData.n) : ""
                        role: "code"
                        size: Theme.typography.caption.size
                        color: AgentPalette.lineNo
                        horizontalAlignment: Text.AlignRight
                    }
                }
                Txt {
                    x: sideways.gutter + 6
                    anchors.verticalCenter: parent.verticalCenter
                    text: pagedLine.kind === "add" ? "+" : (pagedLine.kind === "del" ? "−" : "")
                    role: "code"
                    color: pagedLine.kind === "add" ? AgentPalette.addSign : AgentPalette.delSign
                }
                Text {
                    id: pagedText
                    x: sideways.gutter + 20
                    text: pagedLine.modelData.s
                    // Markup built by the app from escaped text; never the agent's.
                    textFormat: pagedLine.modelData.r === true ? Text.RichText : Text.StyledText
                    wrapMode: Text.NoWrap
                    color: pagedLine.kind === "hunk" ? AgentPalette.hunk : AgentPalette.diffText
                    font.family: Theme.fontMono
                    font.features: Theme.monoFeatures
                    font.pixelSize: Theme.typography.code.size
                }
            }
            TouchpadGlide { view: pagedRows }
        }
    }
    // Beside the view, not in its moving content.
    G7ScrollBar {
        visible: diffView.paged
        flickable: sideways
        orientation: Qt.Horizontal
        anchors.left: sideways.left
        anchors.right: sideways.right
        anchors.bottom: sideways.bottom
    }
    G7ScrollBar {
        visible: diffView.paged
        flickable: pagedRows
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.right: parent.right
    }

    Column {
        id: column
        visible: !diffView.paged
        width: diffView.width
        spacing: 6
        Repeater {
            model: diffView.paged ? [] : diffView.files
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
                            color: AgentPalette.inserted
                        }
                        Txt {
                            text: "−" + (file.modelData.dels || 0)
                            role: "caption"
                            color: AgentPalette.deleted
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
                                color: kind === "add" ? AgentPalette.addRow
                                    : kind === "del" ? AgentPalette.delRow
                                    : kind === "hunk" ? AgentPalette.hunkGround : "transparent"
                                Row {
                                    visible: file.numbered && line.kind !== "hunk"
                                    x: 4
                                    anchors.verticalCenter: parent.verticalCenter
                                    Txt {
                                        width: file.digits * 8 + 10
                                        text: (line.modelData.o || 0) > 0 ? String(line.modelData.o) : ""
                                        role: "code"
                                        size: Theme.typography.caption.size
                                        color: AgentPalette.lineNo
                                        horizontalAlignment: Text.AlignRight
                                    }
                                    Txt {
                                        width: file.digits * 8 + 10
                                        text: (line.modelData.n || 0) > 0 ? String(line.modelData.n) : ""
                                        role: "code"
                                        size: Theme.typography.caption.size
                                        color: AgentPalette.lineNo
                                        horizontalAlignment: Text.AlignRight
                                    }
                                }
                                Txt {
                                    x: line.gutter + 6
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: line.kind === "add" ? "+" : (line.kind === "del" ? "−" : "")
                                    role: "code"
                                    color: line.kind === "add" ? AgentPalette.addSign : AgentPalette.delSign
                                }
                                Text {
                                    id: lineText
                                    x: line.gutter + 20
                                    text: line.modelData.s
                                    // Markup built by the app from escaped text; never the agent's.
                                    textFormat: line.modelData.r === true ? Text.RichText : Text.StyledText
                                    wrapMode: Text.NoWrap
                                    color: line.kind === "hunk" ? AgentPalette.hunk : AgentPalette.diffText
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
