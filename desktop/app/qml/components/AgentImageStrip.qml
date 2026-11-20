// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The images the next message carries, as the host re-encoded them, each
// with a remove; and why the last one wasn't added.
Column {
    id: strip
    // `composer.pending`: `[{ id, name, thumb }]`.
    property var images: []
    // Images still being added.
    property int adding: 0
    // "type", "size", "model", "count", "failed" or "".
    property string error
    property string agentName
    signal removed(string id)

    readonly property string errorText: {
        switch (error) {
        case "type": return qsTr("That isn't an image G7 can send.")
        case "size": return qsTr("That image is too large.")
        case "model": return qsTr("%1 can't take images with this model.").arg(agentName)
        case "count": return qsTr("Up to 4 images a message.")
        case "failed": return qsTr("Couldn't add that image.")
        default: return ""
        }
    }

    spacing: 6

    Row {
        visible: strip.images.length + strip.adding > 0
        spacing: 8
        Repeater {
            model: strip.images
            delegate: Item {
                id: thumb
                required property var modelData
                width: 52
                height: 52
                Accessible.role: Accessible.Graphic
                Accessible.name: thumb.modelData.name
                Rectangle {
                    y: 4
                    width: 48
                    height: 48
                    radius: Theme.radiusSm
                    color: Theme.surfaceContainerHigh
                    border.width: 1
                    border.color: Theme.outlineVariant
                    clip: true
                    // A JPEG the host made: never the original file's bytes.
                    Image {
                        anchors.fill: parent
                        source: thumb.modelData.thumb
                        fillMode: Image.PreserveAspectCrop
                        sourceSize.width: 96
                        sourceSize.height: 96
                        asynchronous: true
                    }
                    Icon {
                        visible: thumb.modelData.thumb.length === 0
                        anchors.centerIn: parent
                        width: 18
                        height: 18
                        path: Icons.photo
                        color: Theme.surfaceContentVariant
                    }
                }
                Rectangle {
                    x: 36
                    width: 16
                    height: 16
                    radius: 8
                    color: Theme.surfaceContainerHighest
                    border.width: 1
                    border.color: Theme.outlineVariant
                    Icon {
                        anchors.centerIn: parent
                        width: 10
                        height: 10
                        path: Icons.close
                        color: Theme.surfaceContent
                    }
                    MouseArea {
                        anchors.fill: parent
                        anchors.margins: -4
                        cursorShape: Qt.PointingHandCursor
                        onClicked: strip.removed(thumb.modelData.id)
                    }
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Remove %1").arg(thumb.modelData.name)
                    Accessible.onPressAction: strip.removed(thumb.modelData.id)
                }
            }
        }
        Repeater {
            model: strip.adding
            delegate: Rectangle {
                y: 4
                width: 48
                height: 48
                radius: Theme.radiusSm
                color: Theme.surfaceContainerHigh
                Spinner {
                    anchors.centerIn: parent
                    width: 16
                    height: 16
                }
            }
        }
    }
    Row {
        visible: strip.errorText.length > 0
        spacing: 6
        Icon {
            anchors.verticalCenter: parent.verticalCenter
            width: 14
            height: 14
            path: Icons.warning
            color: Theme.error
        }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            text: strip.errorText
            role: "bodySmall"
            color: Theme.error
        }
    }
}
