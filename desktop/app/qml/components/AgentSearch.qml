// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Search in the thread list: the messages that match, grouped by thread,
// newest first. Under three letters only titles are searched, and it says
// so. A switch includes archived threads.
Column {
    id: results
    // Agents.search, parsed.
    property var search: ({})
    property real now: Date.now()
    property string openId

    signal open(string thread, string seq)
    signal archivedToggled(bool on)

    readonly property var groups: search.groups || []
    readonly property bool loading: search.loading === true

    Item {
        width: results.width
        height: 32
        Txt {
            x: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Include archived")
            role: "bodySmall"
        }
        Toggle {
            anchors.right: parent.right
            anchors.rightMargin: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            label: qsTr("Include archived")
            checked: results.search.archived === true
            onToggled: (on) => results.archivedToggled(on)
        }
    }
    Txt {
        visible: results.search.titlesOnly === true
        x: Theme.rowPadding
        width: results.width - 2 * Theme.rowPadding
        topPadding: 4
        bottomPadding: 6
        text: qsTr("Type 3 letters or more to search messages.")
        role: "bodySmall"
        muted: true
        wrapMode: Text.WordWrap
        elide: Text.ElideNone
    }
    Item {
        visible: results.loading && results.groups.length === 0
        width: results.width
        height: visible ? 36 : 0
        Spinner {
            anchors.centerIn: parent
            width: 16
            height: 16
        }
    }
    Txt {
        visible: !results.loading && results.groups.length === 0 && results.search.titlesOnly !== true
        x: Theme.rowPadding
        width: results.width - 2 * Theme.rowPadding
        topPadding: 8
        text: results.search.failed === true ? qsTr("Something went wrong. Try again.") : qsTr("No messages match")
        role: "bodySmall"
        muted: true
    }
    Repeater {
        model: results.groups
        delegate: AgentHitRow {
            required property var modelData
            width: results.width
            group: modelData
            now: results.now
            openId: results.openId
            onOpen: (thread, seq) => results.open(thread, seq)
        }
    }
}
