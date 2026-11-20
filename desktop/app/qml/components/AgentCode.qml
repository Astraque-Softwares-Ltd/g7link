// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A code block the agent wrote: its language, Wrap and Copy, and its lines
// coloured by kind (markup the app built from escaped text). Unwrapped it
// scrolls sideways. Code keeps every character in its own cell: ligatures
// are off, as everywhere code shows.
Item {
    id: code
    required property string key
    required property int depth
    required property int quote
    required property string styled
    required property string lang
    required property bool wrapped
    required property int lines

    // Past this many lines each line is its own row, so a long block isn't
    // laid out as one text.
    readonly property int longLines: 200
    readonly property bool isLong: lines > longLines
    readonly property var lineList: isLong ? styled.split("<br>") : []

    width: ListView.view ? ListView.view.width : 0
    height: box.height + 10

    Accessible.role: Accessible.Grouping
    Accessible.name: lang.length > 0 ? qsTr("%1 code").arg(lang) : qsTr("Code")

    Rectangle {
        id: box
        x: code.depth * 18 + code.quote * 14
        y: 5
        width: parent.width - x
        height: head.height + body.height + 10
        radius: Theme.radiusMd
        color: Theme.surfaceContainerHigh

        Item {
            id: head
            width: parent.width
            height: 28
            Txt {
                x: 12
                anchors.verticalCenter: parent.verticalCenter
                text: code.lang
                role: "caption"
                muted: true
            }
            Row {
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Wrap")
                    selected: code.wrapped
                    iconColor: Theme.surfaceContentVariant
                    onClicked: Agents.toggleWrap(code.key)
                }
                TextAction {
                    id: copy
                    property bool done: false
                    anchors.verticalCenter: parent.verticalCenter
                    text: done ? qsTr("Copied") : qsTr("Copy")
                    iconPath: done ? Icons.check : Icons.copy
                    iconColor: Theme.surfaceContentVariant
                    onClicked: {
                        if (Agents.copyText(code.key)) {
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
        }

        Flickable {
            id: body
            visible: !code.isLong
            x: 12
            y: head.height
            width: parent.width - 24
            height: visible ? source.height : 0
            contentWidth: code.wrapped ? width : Math.max(width, source.implicitWidth)
            contentHeight: source.height
            flickableDirection: Flickable.HorizontalFlick
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            interactive: contentWidth > width
            Text {
                id: source
                objectName: "source"
                width: code.wrapped ? body.width : implicitWidth
                text: code.isLong ? "" : code.styled
                // Markup built by the app from escaped text; never the agent's.
                textFormat: Text.StyledText
                wrapMode: code.wrapped ? Text.WrapAnywhere : Text.NoWrap
                color: Theme.surfaceContent
                font.family: Theme.fontMono
                font.features: Theme.monoFeatures
                font.pixelSize: Theme.typography.code.size
            }
        }

        G7ScrollBar {
            flickable: body
            orientation: Qt.Horizontal
            anchors.left: body.left
            anchors.right: body.right
            anchors.bottom: body.bottom
            anchors.bottomMargin: -6
        }

        // A long block: one row a line, in its own scroll.
        ListView {
            id: longView
            visible: code.isLong
            x: 12
            y: head.height
            width: parent.width - 24
            height: visible ? Math.min(contentHeight, 480) : 0
            model: code.lineList
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            contentWidth: code.wrapped ? width : Math.max(width, longView.widest)
            flickableDirection: Flickable.AutoFlickIfNeeded
            property real widest: 0
            delegate: Text {
                id: lineText
                required property string modelData
                width: code.wrapped ? longView.width : implicitWidth
                text: modelData
                textFormat: Text.StyledText
                wrapMode: code.wrapped ? Text.WrapAnywhere : Text.NoWrap
                color: Theme.surfaceContent
                font.family: Theme.fontMono
                font.features: Theme.monoFeatures
                font.pixelSize: Theme.typography.code.size
                Component.onCompleted: longView.widest = Math.max(longView.widest, implicitWidth)
            }
        }
        G7ScrollBar {
            flickable: longView
            anchors.top: longView.top
            anchors.bottom: longView.bottom
            anchors.right: longView.right
        }
        G7ScrollBar {
            flickable: longView
            orientation: Qt.Horizontal
            anchors.left: longView.left
            anchors.right: longView.right
            anchors.bottom: longView.bottom
            anchors.bottomMargin: -6
        }
    }
    // The hairline of an opened group or fold: the only nesting mark.
    Rectangle {
        visible: code.depth > 0
        x: (code.depth - 1) * 18 + 8
        width: 1
        height: parent.height
        color: Theme.line
    }
}
