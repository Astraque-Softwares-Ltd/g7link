// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// "All folders on <PC>" in the new thread's Where: Home and the mounted
// disks, opened a level at a time (folders only, no hidden ones, read from
// the agent host 200 at a time), repositories marked with the branch glyph
// and your agent folders with a check. A click opens or closes a folder;
// "Use" (or Enter, or a double-click) chooses it: one of your folders is
// selected, any other is added at once. A disk itself only opens.
// Keys: Up and Down move, Right opens, Left closes or goes to the parent.
FocusScope {
    id: tree
    // Agents.places, parsed: { "<place>": { entries: [{ name, repo, folderId }], more, loading, failed } }.
    property var places: ({})
    // The view it scrolls in, to keep the keyboard's row in sight.
    property Flickable flickable
    // A folder was chosen: `folderId` when it's one of yours, else `place` to add.
    signal use(string place, string folderId)

    // Places opened, by place.
    property var expanded: ({})
    property int current: 0

    // Starts afresh, with the roots read again (each time the form opens).
    function reset() {
        expanded = ({})
        current = 0
        Agents.readPlaces("", 0)
    }
    function toggle(place) {
        const next = Object.assign({}, expanded)
        if (rowOpen(place)) {
            delete next[place]
        } else {
            next[place] = true
            if (!places[place] || places[place].failed === true)
                Agents.readPlaces(place, 0)
        }
        expanded = next
    }
    function rowOpen(place) {
        return expanded[place] === true && places[place] !== undefined
    }
    function choose(row) {
        if (row.kind === "place" && !row.root)
            tree.use(row.place, row.folderId)
        else if (row.kind === "place")
            toggle(row.place)
    }
    function activate(row) {
        switch (row.kind) {
        case "place": toggle(row.place); break
        case "more": Agents.readPlaces(row.place, row.offset); break
        case "failed": Agents.readPlaces(row.place, 0); break
        }
    }

    // The tree as rows, in order: each place, its folders under it while
    // it's open, then "Loading…", "Show more" or why it couldn't be read.
    readonly property var rows: {
        const out = []
        const walk = (place, depth) => {
            const list = tree.places[place]
            if (!list)
                return
            for (const e of list.entries || []) {
                const child = place.length > 0 ? place + "/" + e.name : e.name
                const open = tree.rowOpen(child)
                out.push({ kind: "place", place: child, name: e.name, depth: depth, repo: e.repo === true,
                           folderId: e.folderId || "", root: depth === 0, open: open })
                if (open)
                    walk(child, depth + 1)
            }
            const name = place.length > 0 ? place.split("/").pop() : ""
            if (list.loading)
                out.push({ kind: "loading", place: place, depth: depth })
            else if (list.failed)
                out.push({ kind: "failed", place: place, depth: depth, name: name })
            else if (list.more)
                out.push({ kind: "more", place: place, depth: depth, offset: (list.entries || []).length })
        }
        walk("", 0)
        return out
    }
    onRowsChanged: current = Math.min(current, Math.max(0, rows.length - 1))

    function reveal(i) {
        const item = rowRepeater.itemAt(i)
        if (!item || !tree.flickable)
            return
        const p = item.mapToItem(tree.flickable.contentItem, 0, 0)
        if (p.y < tree.flickable.contentY)
            tree.flickable.contentY = p.y
        else if (p.y + item.height > tree.flickable.contentY + tree.flickable.height)
            tree.flickable.contentY = p.y + item.height - tree.flickable.height
    }
    function move(by) {
        current = Math.max(0, Math.min(rows.length - 1, current + by))
        reveal(current)
    }

    implicitHeight: column.height
    activeFocusOnTab: rows.length > 0
    Accessible.role: Accessible.Tree
    Keys.onPressed: (event) => {
        const row = tree.rows[tree.current]
        if (!row)
            return
        switch (event.key) {
        case Qt.Key_Down: tree.move(1); break
        case Qt.Key_Up: tree.move(-1); break
        case Qt.Key_Right:
            if (row.kind === "place" && !row.open)
                tree.toggle(row.place)
            break
        case Qt.Key_Left:
            if (row.kind === "place" && row.open) {
                tree.toggle(row.place)
            } else {
                const parent = row.place.split("/").slice(0, row.kind === "place" ? -1 : undefined).join("/")
                const at = tree.rows.findIndex((r) => r.kind === "place" && r.place === parent)
                if (at >= 0) {
                    tree.current = at
                    tree.reveal(at)
                }
            }
            break
        case Qt.Key_Return:
        case Qt.Key_Enter:
            if (row.kind === "place")
                tree.choose(row)
            else
                tree.activate(row)
            break
        case Qt.Key_Space: tree.activate(row); break
        default: return
        }
        event.accepted = true
    }

    Column {
        id: column
        width: parent.width
        Repeater {
            id: rowRepeater
            model: tree.rows
            delegate: Item {
                id: rowItem
                required property var modelData
                required property int index
                readonly property bool isPlace: modelData.kind === "place"
                readonly property bool lit: hover.hovered || (tree.activeFocus && tree.current === index)
                width: column.width
                height: 28
                Accessible.role: isPlace ? Accessible.TreeItem : Accessible.Button
                Accessible.name: isPlace ? modelData.name : rowText.text
                Accessible.onPressAction: tree.activate(modelData)

                Rectangle {
                    anchors.fill: parent
                    radius: Theme.radiusSm
                    color: Theme.raised
                    visible: rowItem.lit && rowItem.modelData.kind !== "loading"
                }
                Rectangle {
                    anchors.fill: parent
                    radius: Theme.radiusSm
                    color: "transparent"
                    border.width: 1
                    border.color: Theme.focusRing
                    visible: tree.activeFocus && tree.current === rowItem.index && Theme.focusVisible(tree)
                }
                Row {
                    x: 4 + rowItem.modelData.depth * 16
                    width: parent.width - x - (useAction.visible ? useAction.width + 8 : (doneIcon.visible ? 26 : 4))
                    height: parent.height
                    spacing: 4
                    Icon {
                        visible: rowItem.isPlace
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16
                        height: 16
                        path: rowItem.modelData.open ? Icons.chevronDown : Icons.chevronRight
                        color: Theme.surfaceContentVariant
                    }
                    Icon {
                        id: kindIcon
                        visible: rowItem.isPlace
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        path: rowItem.modelData.root ? (rowItem.modelData.name === "Home" ? Icons.home : Icons.folder)
                            : rowItem.modelData.repo ? AgentGitGlyphs.branch : Icons.folder
                        color: rowItem.modelData.repo ? Theme.accent : Theme.tintAmber
                    }
                    Item {
                        visible: !rowItem.isPlace
                        width: 20
                        height: 1
                    }
                    Txt {
                        id: rowText
                        width: parent.width - (rowItem.isPlace ? 16 + kindIcon.width + 2 * parent.spacing : 24)
                        anchors.verticalCenter: parent.verticalCenter
                        text: {
                            switch (rowItem.modelData.kind) {
                            case "loading": return qsTr("Loading…")
                            case "more": return qsTr("Show more")
                            case "failed": return (rowItem.modelData.name.length > 0
                                                   ? qsTr("Couldn't read %1").arg(rowItem.modelData.name)
                                                   : qsTr("Couldn't reach the agent host")) + " · " + qsTr("Try again")
                            default: return rowItem.modelData.name
                            }
                        }
                        role: "bodySmall"
                        muted: !rowItem.isPlace
                        color: rowItem.modelData.kind === "more" ? Theme.accent : (muted ? Theme.surfaceContentVariant : Theme.surfaceContent)
                        maximumLineCount: 1
                    }
                }
                Icon {
                    id: doneIcon
                    visible: rowItem.isPlace && rowItem.modelData.folderId.length > 0 && !useAction.visible
                    anchors.right: parent.right
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                    path: Icons.check
                    color: Theme.accent
                }
                HoverHandler {
                    id: hover
                    enabled: rowItem.modelData.kind !== "loading"
                    cursorShape: Qt.PointingHandCursor
                }
                MouseArea {
                    anchors.fill: parent
                    enabled: rowItem.modelData.kind !== "loading"
                    onClicked: {
                        tree.current = rowItem.index
                        tree.activate(rowItem.modelData)
                    }
                    onDoubleClicked: if (rowItem.isPlace && !rowItem.modelData.root) tree.choose(rowItem.modelData)
                }
                TextAction {
                    id: useAction
                    visible: rowItem.isPlace && !rowItem.modelData.root && rowItem.lit
                    anchors.right: parent.right
                    anchors.rightMargin: 2
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Use")
                    selected: true
                    onClicked: tree.choose(rowItem.modelData)
                }
            }
        }
    }
}
