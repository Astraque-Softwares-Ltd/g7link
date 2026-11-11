// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A section's label: 12 px, muted, sentence case, lined up with the rows
// under it. An optional text action ("Clear all", "New") sits on the right.
Item {
    id: section
    property string text
    property string actionText
    property string actionIcon
    signal actionClicked

    implicitWidth: label.implicitWidth + 2 * Theme.rowPadding
    implicitHeight: 30

    Txt {
        id: label
        anchors.left: parent.left
        anchors.leftMargin: Theme.rowPadding
        anchors.right: action.visible ? action.left : parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        text: section.text
        role: "label"
        muted: true
    }
    TextAction {
        id: action
        visible: section.actionText.length > 0
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        text: section.actionText
        iconPath: section.actionIcon
        onClicked: section.actionClicked()
    }
}
