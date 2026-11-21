// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// One thread in the search results: its title, then up to three messages
// that match with the matches in bold, and "N more in this thread". The
// title opens the thread; a message opens it at that message.
Column {
    id: hits
    // A group of Agents.search: { thread, title, agentName, state, archived,
    // updatedMs, more, hits: [{ seq, who, agentName, when, markup, text }] }.
    property var group: ({})
    property real now: Date.now()
    // The thread the page shows, for the selection.
    property string openId

    signal open(string thread, string seq)

    function when(ms) {
        const date = new Date(ms)
        const time = date.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
        return new Date(hits.now).toDateString() === date.toDateString()
            ? time : date.toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }
    function ago(ms) {
        const minutes = Math.max(0, Math.floor((hits.now - ms) / 60000))
        if (minutes < 1)
            return qsTr("now")
        if (minutes < 60)
            return qsTr("%1 min").arg(minutes)
        const hours = Math.floor(minutes / 60)
        return hours < 24 ? qsTr("%1 h").arg(hours) : qsTr("%1 d").arg(Math.floor(hours / 24))
    }

    component Pressable: Item {
        id: pressable
        property bool selected: false
        signal activated
        activeFocusOnTab: true
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                pressable.activated()
                event.accepted = true
            }
        }
        Accessible.role: Accessible.Button
        Accessible.onPressAction: pressable.activated()
        StateFill {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: Theme.raised
            on: !pressable.selected && pressMouse.containsMouse
        }
        StateFill {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: Theme.selection
            on: pressable.selected
        }
        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: "transparent"
            visible: Theme.focusVisible(pressable)
            border.width: 2
            border.color: Theme.focusRing
        }
        MouseArea {
            id: pressMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: pressable.activated()
        }
    }

    Pressable {
        width: hits.width
        height: 30
        selected: (hits.group.hits || []).length === 0 && hits.openId === hits.group.thread
        Accessible.name: hits.group.title || ""
        onActivated: hits.open(hits.group.thread, "0")
        AgentStateMark {
            x: Theme.rowPadding - 2
            anchors.verticalCenter: parent.verticalCenter
            phase: hits.group.state || ""
        }
        Txt {
            x: Theme.rowPadding + 18
            anchors.right: headEnd.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            text: hits.group.title || ""
            weight: 500
            maximumLineCount: 1
        }
        Row {
            id: headEnd
            anchors.right: parent.right
            anchors.rightMargin: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Txt {
                visible: hits.group.archived === true
                text: qsTr("Archived")
                role: "caption"
                muted: true
            }
            Txt {
                text: hits.ago(hits.group.updatedMs || 0)
                role: "bodySmall"
                muted: true
            }
        }
    }
    Repeater {
        model: hits.group.hits || []
        delegate: Pressable {
            id: hit
            required property var modelData
            width: hits.width
            height: hitColumn.height + 12
            Accessible.name: modelData.text
            onActivated: hits.open(hits.group.thread, hit.modelData.seq)
            Column {
                id: hitColumn
                x: Theme.rowPadding + 18
                y: 6
                width: parent.width - x - Theme.rowPadding
                spacing: 2
                Text {
                    width: parent.width
                    text: hit.modelData.markup
                    // Markup built by the app from escaped text; never the agent's.
                    textFormat: Text.StyledText
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                    color: Theme.surfaceContent
                    font.family: Theme.fontUi
                    font.pixelSize: Theme.typography.bodySmall.size
                }
                Txt {
                    text: (hit.modelData.who === "you" ? qsTr("You")
                           : hit.modelData.who === "agent" ? hit.modelData.agentName : "")
                        + (hit.modelData.who.length > 0 ? " · " : "") + hits.when(hit.modelData.when)
                    role: "caption"
                    muted: true
                }
            }
        }
    }
    Pressable {
        visible: (hits.group.more || 0) > 0
        width: hits.width
        height: visible ? 26 : 0
        Accessible.name: moreText.text
        onActivated: hits.open(hits.group.thread, ((hits.group.hits || [])[0] || {}).seq || "0")
        Row {
            x: Theme.rowPadding + 18
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
            Txt {
                id: moreText
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("%n more in this thread", "", hits.group.more || 0)
                role: "bodySmall"
                weight: 500
            }
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 12
                height: 12
                path: Icons.chevronRight
                color: Theme.surfaceContentVariant
            }
        }
    }
}
