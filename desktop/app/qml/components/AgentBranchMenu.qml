// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The branch menu (§SC9.2): a search, "New branch…" (a name field in
// place), the local branches newest first, then those only on a remote.
// The current one has the check. A branch can't be picked while the thread
// works, when it's open in another worktree, or when the thread works in
// its own worktree; the row says why. Picking one with changes in the
// folder asks first (`switchAsked`).
Item {
    id: menu
    // Agents.git's branches: { open, query, loading, more, pushAfter, list: [{ name, remote, current, worktree }] }
    property var branches: ({})
    property string agentName
    // Why no branch can be picked ("" when one can): "running" or "worktree".
    property string locked
    // Files with changes in the folder.
    property int changed: 0
    property real anchorX: 0
    property real anchorY: 0
    signal switchAsked(string branch)

    readonly property bool opened: branches.open === true
    readonly property var list: branches.list || []
    readonly property var locals: list.filter((b) => (b.remote || "").length === 0)
    readonly property var remotes: list.filter((b) => (b.remote || "").length > 0)
    property bool naming: false
    readonly property string name: nameField.text.trim()
    readonly property string nameProblem: name.length === 0 ? ""
        : !Agents.branchNameOk(name) ? qsTr("That name can't be a branch.")
        : list.some((b) => b.name === name && (b.remote || "").length === 0) ? qsTr("A branch with that name exists.") : ""

    function shownName(b) {
        const remote = b.remote || ""
        return remote.length > 0 && !b.name.startsWith(remote + "/") ? remote + "/" + b.name : b.name
    }
    function why(b) {
        if (menu.locked === "running")
            return qsTr("Wait until %1 finishes.").arg(menu.agentName)
        if (menu.locked === "worktree")
            return qsTr("This thread works in its own worktree.")
        if ((b.worktree || "").length > 0 && b.current !== true)
            return qsTr("Open in another worktree (%1)").arg(b.worktree)
        return ""
    }
    function pick(b) {
        if (b.current === true || why(b).length > 0)
            return
        if (menu.changed > 0)
            menu.switchAsked(b.name)
        else
            Agents.gitSwitch(b.name, false, false)
    }
    function create() {
        if (menu.name.length === 0 || menu.nameProblem.length > 0 || menu.locked.length > 0)
            return
        Agents.gitSwitch(menu.name, true, false)
        nameField.text = ""
        menu.naming = false
    }
    function ago(ms) {
        const s = Math.max(0, (Date.now() - ms) / 1000)
        if (ms <= 0)
            return ""
        if (s < 3600)
            return qsTr("%1 min").arg(Math.max(1, Math.floor(s / 60)))
        if (s < 86400)
            return qsTr("%1 h").arg(Math.floor(s / 3600))
        if (s < 14 * 86400)
            return qsTr("%1 d").arg(Math.floor(s / 86400))
        return qsTr("%1 w").arg(Math.floor(s / (7 * 86400)))
    }

    anchors.fill: parent
    visible: opened
    z: 90
    onOpenedChanged: {
        if (opened) {
            menu.naming = (branches.pushAfter === true)
            search.text = ""
            Qt.callLater(() => (menu.naming ? nameField : search).input.forceActiveFocus())
        }
    }

    // A click outside closes it.
    MouseArea {
        anchors.fill: parent
        onClicked: Agents.gitBranchMenu(false)
    }
    Keys.onEscapePressed: Agents.gitBranchMenu(false)

    Rectangle {
        id: card
        x: Math.max(8, Math.min(menu.anchorX, menu.width - width - 8))
        y: Math.max(8, Math.min(menu.anchorY, menu.height - height - 8))
        width: 320
        height: Math.min(content.height + 16, menu.height - 16)
        radius: Theme.radiusLg
        color: Theme.surfaceContainerHigh
        border.width: 1
        border.color: Theme.outlineVariant
        clip: true
        // Clicks inside stay inside.
        MouseArea {
            anchors.fill: parent
        }

        Flickable {
            anchors.fill: parent
            anchors.margins: 8
            contentHeight: content.height
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            Column {
                id: content
                width: parent.width
                spacing: 2
                Field {
                    id: search
                    width: parent.width
                    placeholder: qsTr("Find a branch")
                    iconPath: Icons.search
                    onTextChanged: Agents.gitBranchQuery(search.text)
                    Keys.onEscapePressed: Agents.gitBranchMenu(false)
                }
                ActionRow {
                    visible: !menu.naming
                    width: parent.width
                    iconPath: Icons.plus
                    tint: Theme.accent
                    title: qsTr("New branch…")
                    enabled: menu.locked !== "worktree"
                    onClicked: {
                        menu.naming = true
                        Qt.callLater(() => nameField.input.forceActiveFocus())
                    }
                }
                Column {
                    visible: menu.naming
                    width: parent.width
                    spacing: 4
                    Item {
                        width: parent.width
                        height: nameField.height
                        Field {
                            id: nameField
                            anchors.left: parent.left
                            anchors.right: createButton.left
                            anchors.rightMargin: 6
                            placeholder: qsTr("Branch name")
                            onAccepted: menu.create()
                            Keys.onEscapePressed: menu.naming = false
                        }
                        Button {
                            id: createButton
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            size: "sm"
                            text: qsTr("Create")
                            enabled: menu.name.length > 0 && menu.nameProblem.length === 0 && menu.locked.length === 0
                            onClicked: menu.create()
                        }
                    }
                    Txt {
                        width: parent.width
                        visible: text.length > 0
                        text: menu.nameProblem.length > 0 ? menu.nameProblem
                            : menu.changed > 0 ? qsTr("Your changes come along.") : ""
                        role: "caption"
                        color: menu.nameProblem.length > 0 ? AgentPalette.failedText : Theme.surfaceContentVariant
                        wrapMode: Text.WordWrap
                    }
                }
                Spinner {
                    visible: menu.branches.loading === true
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 16
                    height: 16
                }
                Repeater {
                    model: menu.locals
                    delegate: branchRow
                }
                Txt {
                    visible: menu.remotes.length > 0
                    x: 8
                    height: 26
                    verticalAlignment: Text.AlignVCenter
                    text: qsTr("On %1").arg((menu.remotes[0] || {}).remote || "")
                    role: "label"
                    muted: true
                }
                Repeater {
                    model: menu.remotes
                    delegate: branchRow
                }
            }
        }
    }

    Component {
        id: branchRow
        Item {
            id: rowItem
            required property var modelData
            readonly property string reason: menu.why(modelData)
            width: content.width
            height: 34
            opacity: reason.length > 0 ? 0.6 : 1
            activeFocusOnTab: reason.length === 0
            Accessible.role: Accessible.MenuItem
            Accessible.name: menu.shownName(modelData)
            Accessible.description: reason
            Accessible.checkable: true
            Accessible.checked: modelData.current === true
            Accessible.onPressAction: menu.pick(rowItem.modelData)
            Keys.onReturnPressed: menu.pick(rowItem.modelData)
            Keys.onEnterPressed: menu.pick(rowItem.modelData)
            Rectangle {
                anchors.fill: parent
                radius: Theme.radiusSm
                color: rowHover.hovered && rowItem.reason.length === 0 ? Theme.selection
                    : rowItem.activeFocus ? Theme.selection : "transparent"
            }
            Icon {
                id: mark
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: rowItem.modelData.current === true ? Icons.check : AgentGitGlyphs.branch
                color: rowItem.modelData.current === true ? Theme.accent : Theme.surfaceContentVariant
            }
            Column {
                anchors.left: mark.right
                anchors.leftMargin: 8
                anchors.right: when.left
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                Txt {
                    width: parent.width
                    text: menu.shownName(rowItem.modelData)
                    role: "code"
                    size: Theme.typography.bodySmall.size
                    maximumLineCount: 1
                }
                Txt {
                    visible: rowItem.reason.length > 0
                    width: parent.width
                    text: rowItem.reason
                    role: "caption"
                    muted: true
                    maximumLineCount: 1
                }
            }
            Txt {
                id: when
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                text: menu.ago(rowItem.modelData.lastMs || 0)
                role: "caption"
                muted: true
            }
            HoverHandler {
                id: rowHover
                cursorShape: rowItem.reason.length === 0 ? Qt.PointingHandCursor : Qt.ArrowCursor
            }
            TapHandler {
                onTapped: menu.pick(rowItem.modelData)
            }
        }
    }
}
