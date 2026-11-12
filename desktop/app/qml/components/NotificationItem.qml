// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import app.g7link

// One phone notification as a 36 px row: the app's icon, who it's from,
// the text and when. Selected, it opens up: the whole text, its picture,
// the app's own actions, Reply, Copy code, Open, Options and Dismiss.
Item {
    id: item
    required property string deviceId
    required property string key
    required property string app
    required property string appName
    required property string title
    required property string text
    required property string sub
    required property real when
    required property string iconUrl
    required property string imageUrl
    required property string actions
    required property string replyAction
    required property string replyLabel
    required property string replies
    required property string otpCode
    required property string live
    // The phone won't dismiss it (ongoing or no-clear), so it has no Dismiss.
    required property bool ongoing
    required property int groupCount
    required property int groupIndex
    required property bool groupExpanded
    required property string groupSummary
    // Bumped by the feed every minute, so relative times stay right.
    property int clock: 0
    property int chronoTick: 0

    readonly property bool isGrouped: groupCount > 1
    readonly property bool isGroupLead: isGrouped && groupIndex === 0
    readonly property bool isGroupChild: isGrouped && groupIndex > 0
    readonly property var buttons: { try { return JSON.parse(actions) } catch (e) { return [] } }
    readonly property var sent: { try { return JSON.parse(replies) } catch (e) { return [] } }
    readonly property var liveData: {
        if (!live || live.length === 0)
            return null
        try {
            return JSON.parse(live)
        } catch (e) {
            return null
        }
    }
    readonly property bool isLive: liveData !== null
    readonly property bool hasProgress: isLive && (Boolean(liveData.indeterminate) || Number(liveData.max) > 0)
    readonly property var liveSegments: isLive && liveData.segments ? liveData.segments : []
    readonly property var livePoints: isLive && liveData.points ? liveData.points : []
    readonly property string liveBadgeText: {
        if (!isLive)
            return ""
        const c = (liveData.chip || "").trim()
        return c.length > 0 ? c : qsTr("LIVE")
    }
    readonly property string liveRightLabel: {
        if (!isLive)
            return ""
        if (liveData.chronometer)
            return chronoText()
        if (!liveData.indeterminate && Number(liveData.max) > 0) {
            const pct = Math.min(100, Math.max(0, Math.round(Number(liveData.progress) * 100 / Number(liveData.max))))
            return pct + "%"
        }
        return ""
    }
    property bool replying: false
    property bool canOpenApp: false
    property bool selected: false
    readonly property bool open: selected || replying
    // A glyph tint of its own, from the app's name, when it has no icon.
    readonly property color tint: {
        const names = ["teal", "blue", "coral", "pink", "green", "lavender", "amber"]
        let h = 0
        for (let i = 0; i < appName.length; i++)
            h = (h * 31 + appName.charCodeAt(i)) % 997
        return Theme.tint(names[h % names.length])
    }
    signal optionsRequested
    signal openAppRequested
    signal selectRequested
    signal imageClicked(string url)

    implicitHeight: Theme.rowHeight + (details.visible ? details.implicitHeight + 6 : 0)
    height: implicitHeight
    activeFocusOnTab: !item.replying
    Accessible.role: Accessible.Button
    Accessible.name: [item.appName, item.title, item.text].filter(s => s.length > 0).join(", ")
    Accessible.onPressAction: if (!item.replying) item.selectRequested()
    Keys.onPressed: (event) => {
        if (item.replying)
            return
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
            item.selectRequested()
            event.accepted = true
        } else if (event.key === Qt.Key_Delete && dismiss.visible) {
            dismiss.clicked()
            event.accepted = true
        }
    }

    readonly property bool windowVisible: Window.window
        && Window.window.visibility !== Window.Hidden
        && Window.window.visibility !== Window.Minimized

    // Ticks once per second only for visible chronometer notifications while the window is shown.
    Timer {
        id: chronoTimer
        interval: 1000
        repeat: true
        running: item.isLive && Boolean(item.liveData.chronometer)
                 && item.visible && item.windowVisible
        onTriggered: item.chronoTick++
    }

    function chronoText() {
        if (!isLive || !liveData.chronometer)
            return ""
        chronoTick
        const target = Number(liveData.when || when)
        const diffMs = liveData.countdown
            ? Math.max(0, target - Date.now())
            : Math.max(0, Date.now() - target)
        const totalSecs = Math.floor(diffMs / 1000)
        const hrs = Math.floor(totalSecs / 3600)
        const mins = Math.floor((totalSecs % 3600) / 60)
        const secs = totalSecs % 60
        const pad = (n) => (n < 10 ? "0" + n : String(n))
        if (hrs > 0)
            return hrs + ":" + pad(mins) + ":" + pad(secs)
        return mins + ":" + pad(secs)
    }

    function segmentFillFraction(index) {
        if (!isLive || liveSegments.length === 0)
            return 0
        const prog = Number(liveData.progress || 0)
        let start = 0
        for (let i = 0; i < index; i++)
            start += Number(liveSegments[i].length || 0)
        const len = Math.max(1, Number(liveSegments[index].length || 1))
        return Math.min(1, Math.max(0, (prog - start) / len))
    }

    function timeText() {
        clock // re-evaluated each minute
        const minutes = Math.round((Date.now() - when) / 60000)
        if (minutes < 1) return qsTr("now")
        if (minutes < 60) return qsTr("%n min", "", minutes)
        const date = new Date(when)
        if (Date.now() - when < 24 * 3600 * 1000) return date.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
        return date.toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }

    function send() {
        if (replyField.text.trim().length === 0)
            return
        NotificationList.reply(item.deviceId, item.key, item.replyAction, replyField.text)
        replyField.text = ""
        item.replying = false
    }

    function dismissIt() {
        if (item.isGroupLead && !item.groupExpanded)
            NotificationList.dismissGroup(item.deviceId, item.app)
        else
            NotificationList.dismiss(item.deviceId, item.key)
    }

    // The row: a click selects it (and shows its actions), a double click
    // opens the app in a window when it can.
    Rectangle {
        id: rowFill
        x: item.isGroupChild ? 18 : 0
        width: parent.width - x
        height: Theme.rowHeight
        radius: Theme.rowRadius
        color: item.open ? Theme.selection : (hover.containsMouse ? Theme.raised : "transparent")
        border.width: Theme.focusVisible(item) ? 2 : 0
        border.color: Theme.focusRing
    }
    MouseArea {
        id: hover
        anchors.fill: rowFill
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: item.selectRequested()
        onDoubleClicked: if (item.canOpenApp) item.openAppRequested()
    }

    // App icon, or a glyph in its tint when the phone didn't send one.
    Item {
        id: icon
        x: rowFill.x + Theme.rowPadding
        y: (Theme.rowHeight - height) / 2
        width: 18; height: 18
        Image {
            id: image
            anchors.fill: parent
            source: item.iconUrl
            sourceSize: Qt.size(36, 36)
            smooth: true
            mipmap: true
            visible: status === Image.Ready
        }
        Icon {
            anchors.fill: parent
            visible: image.status !== Image.Ready
            path: Icons.bell
            color: item.tint
        }
    }
    Txt {
        id: who
        anchors.left: icon.right
        anchors.leftMargin: 10
        y: (Theme.rowHeight - height) / 2
        width: Math.min(implicitWidth, (trail.x - x - (countBadge.visible ? countBadge.width + 6 : 0)) * (line.text.length > 0 ? 0.45 : 1))
        text: item.title.length > 0 ? item.title : item.appName
        weight: 500
    }
    // How many this app's group holds, right after its name.
    Rectangle {
        id: countBadge
        visible: item.isGroupLead
        anchors.left: who.right
        anchors.leftMargin: 6
        y: (Theme.rowHeight - height) / 2
        width: countText.implicitWidth + 10
        height: 18
        radius: 5
        color: Theme.line
        Accessible.role: Accessible.StaticText
        Accessible.name: qsTr("%n notification(s)", "", item.groupCount)
        Txt {
            id: countText
            anchors.centerIn: parent
            text: String(item.groupCount)
            role: "caption"
            muted: true
        }
    }
    Txt {
        id: line
        anchors.left: countBadge.visible ? countBadge.right : who.right
        anchors.leftMargin: 10
        y: (Theme.rowHeight - height) / 2
        width: Math.max(0, trail.x - x - 10)
        text: item.text.replace(/\s*\n\s*/g, " ")
        muted: true
    }
    Row {
        id: trail
        anchors.right: rowFill.right
        anchors.rightMargin: 6
        y: (Theme.rowHeight - height) / 2
        height: 28
        spacing: 6
        TextAction {
            visible: item.otpCode.length > 0 && !item.open
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Copy code")
            onClicked: NotificationList.copyCode(item.otpCode)
        }
        // Where the phone offers a reply: right there, without opening it first.
        TextAction {
            visible: item.replyAction.length > 0 && !item.open && item.otpCode.length === 0
            anchors.verticalCenter: parent.verticalCenter
            text: item.replyLabel.length > 0 ? item.replyLabel : qsTr("Reply")
            onClicked: {
                item.replying = true
                replyField.input.forceActiveFocus()
            }
        }
        Txt {
            visible: item.liveRightLabel.length > 0
            anchors.verticalCenter: parent.verticalCenter
            role: "bodySmall"
            color: Theme.accent
            text: item.liveRightLabel
        }
        Txt {
            visible: item.isLive && item.liveRightLabel.length === 0
            anchors.verticalCenter: parent.verticalCenter
            role: "bodySmall"
            color: Theme.accent
            text: item.liveBadgeText
        }
        // Like Dismiss: always there for keyboards and screen readers, shown
        // on hover.
        IconButton {
            anchors.verticalCenter: parent.verticalCenter
            width: 24; height: 24
            iconSize: 14
            iconPath: Icons.more
            label: qsTr("Options for %1").arg(item.appName)
            opacity: hover.containsMouse || activeFocus || item.open ? 1 : 0
            onClicked: item.optionsRequested()
        }
        IconButton {
            id: dismiss
            anchors.verticalCenter: parent.verticalCenter
            width: 24; height: 24
            iconSize: 14
            iconPath: Icons.close
            visible: !item.ongoing || (item.isGroupLead && !item.groupExpanded && item.groupCount > 1)
            label: item.isGroupLead && !item.groupExpanded
                   ? qsTr("Dismiss %1 notifications from %2").arg(item.groupCount).arg(item.appName)
                   : qsTr("Dismiss")
            opacity: hover.containsMouse || activeFocus || item.open ? 1 : 0
            onClicked: item.dismissIt()
        }
        Txt {
            visible: !item.isLive
            anchors.verticalCenter: parent.verticalCenter
            role: "bodySmall"
            muted: true
            text: item.timeText()
        }
    }

    // ---- Opened: the rest of it, and what can be done ----
    Column {
        id: details
        visible: item.open || item.hasProgress || item.sent.length > 0 || item.isGroupLead
        x: icon.x + icon.width + 10
        y: Theme.rowHeight + 2
        width: parent.width - x - Theme.rowPadding
        spacing: 6

        Txt {
            width: parent.width
            visible: item.open && (line.truncated || item.text.indexOf("\n") >= 0 || item.sub.length > 0)
            role: "bodySmall"
            muted: true
            text: [item.appName, item.sub].filter(s => s.length > 0).join(" · ")
        }
        Txt {
            width: parent.width
            visible: item.open && (line.truncated || item.text.indexOf("\n") >= 0)
            text: item.text
            wrapMode: Text.Wrap
            maximumLineCount: 8
        }

        // Live progress: one smooth bar, or segments with milestone points.
        Item {
            id: liveBarBox
            width: parent.width
            height: 10
            visible: item.hasProgress

            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 4
                radius: 2
                visible: item.liveSegments.length === 0
                color: Theme.selection
                Rectangle {
                    height: parent.height
                    radius: 2
                    x: item.isLive && item.liveData.indeterminate ? Math.round(parent.width * 0.15) : 0
                    width: {
                        if (!item.isLive)
                            return 0
                        if (item.liveData.indeterminate)
                            return Math.max(24, Math.round(parent.width * 0.35))
                        const maxVal = Math.max(1, Number(item.liveData.max || 1))
                        const ratio = Math.min(1, Math.max(0, Number(item.liveData.progress || 0) / maxVal))
                        return ratio > 0 ? Math.max(4, Math.round(parent.width * ratio)) : 0
                    }
                    color: Theme.accent
                    Behavior on width {
                        enabled: !Theme.reduceMotion
                        NumberAnimation { duration: 180; easing.type: Easing.OutCubic }
                    }
                }
            }
            Row {
                id: segRow
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 4
                spacing: 3
                visible: item.liveSegments.length > 0
                readonly property real totalLen: {
                    let s = 0
                    for (let i = 0; i < item.liveSegments.length; i++)
                        s += Math.max(1, Number(item.liveSegments[i].length || 1))
                    return Math.max(1, s)
                }
                readonly property real availW: Math.max(10, width - spacing * Math.max(0, item.liveSegments.length - 1))
                Repeater {
                    model: item.liveSegments
                    delegate: Rectangle {
                        required property var modelData
                        required property int index
                        height: 4
                        radius: 2
                        width: Math.max(4, Math.floor(segRow.availW * Math.max(1, Number(modelData.length || 1)) / segRow.totalLen))
                        color: Theme.selection
                        Rectangle {
                            height: parent.height
                            radius: 2
                            width: Math.round(parent.width * item.segmentFillFraction(index))
                            color: modelData.color && modelData.color.length > 0 ? modelData.color : Theme.accent
                        }
                    }
                }
            }
            Repeater {
                model: item.livePoints
                delegate: Rectangle {
                    required property var modelData
                    readonly property real maxVal: Math.max(1, Number(item.isLive ? item.liveData.max : 1))
                    readonly property real posRatio: Math.min(1, Math.max(0, Number(modelData.position || 0) / maxVal))
                    readonly property bool reached: item.isLive && Number(item.liveData.progress || 0) >= Number(modelData.position || 0)
                    width: 8; height: 8; radius: 4
                    anchors.verticalCenter: parent.verticalCenter
                    x: Math.round((liveBarBox.width - width) * posRatio)
                    color: reached
                        ? (modelData.color && modelData.color.length > 0 ? modelData.color : Theme.accent)
                        : Theme.selection
                    border.width: 1.5
                    border.color: modelData.color && modelData.color.length > 0 ? modelData.color : Theme.accent
                }
            }
        }

        // The picture it shows (a photo in a message), whole, as the phone
        // shows it opened: across the row, or as tall as fits and narrower.
        RoundedImage {
            readonly property real aspect: implicitImageWidth > 0 ? implicitImageHeight / implicitImageWidth : 0
            readonly property real maxHeight: 360
            width: aspect > 0 ? Math.min(parent.width, maxHeight / aspect) : parent.width
            height: status === Image.Ready ? width * aspect : 0
            fillMode: Image.PreserveAspectFit
            visible: item.open && item.imageUrl.length > 0 && status === Image.Ready
            source: item.open ? item.imageUrl : ""
            sourceSize.width: 1080
            HoverHandler { cursorShape: Qt.PointingHandCursor }
            TapHandler { onTapped: item.imageClicked(item.imageUrl) }
        }

        // Replies sent from this PC.
        Repeater {
            model: item.sent
            delegate: Row {
                required property var modelData
                width: details.width
                spacing: 6
                opacity: modelData.pending ? 0.55 : 1
                Icon {
                    width: 14; height: 14
                    y: Math.round((replyText.lineHeight - height) / 2)
                    path: modelData.pending ? Icons.send : Icons.check
                    color: Theme.accent
                }
                Txt {
                    id: replyText
                    width: parent.width - 20
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.Wrap
                    maximumLineCount: 3
                    text: modelData.pending ? qsTr("Sending: %1").arg(modelData.text) : qsTr("You: %1").arg(modelData.text)
                }
            }
        }

        Flow {
            width: parent.width
            spacing: 6
            visible: item.open && !item.replying
            Button {
                visible: item.replyAction.length > 0
                variant: "tonal"
                size: "sm"
                text: item.replyLabel.length > 0 ? item.replyLabel : qsTr("Reply")
                onClicked: { item.replying = true; replyField.input.forceActiveFocus() }
            }
            Button {
                visible: item.otpCode.length > 0
                variant: "tonal"
                size: "sm"
                text: qsTr("Copy code")
                onClicked: NotificationList.copyCode(item.otpCode)
            }
            Repeater {
                model: item.buttons
                delegate: Button {
                    required property var modelData
                    variant: "text"
                    size: "sm"
                    text: modelData.title
                    onClicked: NotificationList.runAction(item.deviceId, item.key, modelData.id)
                }
            }
            Button {
                visible: item.canOpenApp
                variant: "text"
                size: "sm"
                text: qsTr("Open")
                Accessible.description: qsTr("Open %1 in an app window").arg(item.appName)
                onClicked: item.openAppRequested()
            }
            Button {
                visible: dismiss.visible
                variant: "text"
                size: "sm"
                text: qsTr("Dismiss")
                onClicked: item.dismissIt()
            }
        }

        // Inline reply.
        Item {
            width: parent.width
            height: 32
            visible: item.replying
            Field {
                id: replyField
                anchors.left: parent.left
                anchors.right: sendButton.left
                anchors.rightMargin: 6
                height: parent.height
                placeholder: qsTr("Reply to %1").arg(item.title.length > 0 ? item.title : item.appName)
                onAccepted: item.send()
                Keys.onEscapePressed: { replyField.text = ""; item.replying = false }
                Accessible.name: qsTr("Reply to %1").arg(item.title)
            }
            Button {
                id: sendButton
                anchors.right: parent.right
                height: parent.height
                size: "sm"
                text: qsTr("Send")
                enabled: replyField.text.trim().length > 0
                onClicked: item.send()
            }
        }

        // An app with 2 or more notifications.
        Row {
            visible: item.isGroupLead
            spacing: 6
            TextAction {
                iconPath: item.groupExpanded ? Icons.chevronUp : Icons.chevronDown
                iconColor: Theme.surfaceContentVariant
                text: item.groupExpanded ? qsTr("Show less") : qsTr("+%1 more").arg(item.groupCount - 1)
                onClicked: NotificationList.toggleGroup(item.app)
            }
            TextAction {
                text: qsTr("Clear %1").arg(item.groupCount)
                onClicked: NotificationList.dismissGroup(item.deviceId, item.app)
            }
        }
    }
}
