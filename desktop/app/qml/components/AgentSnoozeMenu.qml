// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Snooze: out of the main list until a time, or until it needs you. In an
// hour, this evening (18:00, while that's ahead), tomorrow (09:00), next
// week (Monday 09:00), and, while the agent reports one, when its usage
// limit resets. A snooze only hides the thread; nothing runs when it ends.
PopupMenu {
    id: snooze
    // The threads it snoozes.
    property var ids: []
    // The agent whose limit reset is offered ("" for none), and when (ms).
    property string agentName
    property real limitResets: 0
    // Date.now() when it opened.
    property real now: Date.now()

    signal snoozed(var ids, real until)

    function time(date) {
        return date.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
    }
    function at(days, hour) {
        const when = new Date(now)
        when.setDate(when.getDate() + days)
        when.setHours(hour, 0, 0, 0)
        return when
    }
    readonly property var evening: at(0, 18)
    readonly property var tomorrow: at(1, 9)
    // The coming Monday (a week on, on a Monday).
    readonly property var monday: at(((8 - new Date(now).getDay()) % 7) || 7, 9)

    function pick(until) {
        snooze.snoozed(snooze.ids, until)
    }

    // Opens at (x, y), upward when `up`.
    function show(x, y, up) {
        now = Date.now()
        openAt(x, y, up, false)
    }

    panelWidth: 300
    actions: {
        const list = [
            { label: qsTr("In an hour"), keys: time(new Date(now + 3600000)), run: () => snooze.pick(snooze.now + 3600000) }
        ]
        if (evening.getTime() > now + 15 * 60000)
            list.push({ label: qsTr("This evening"), keys: time(evening), run: () => snooze.pick(snooze.evening.getTime()) })
        list.push({ label: qsTr("Tomorrow"), keys: time(tomorrow), run: () => snooze.pick(snooze.tomorrow.getTime()) })
        list.push({ label: qsTr("Next week"), keys: monday.toLocaleDateString(Qt.locale(), "ddd") + " " + time(monday),
                    run: () => snooze.pick(snooze.monday.getTime()) })
        if (agentName.length > 0 && limitResets > now)
            list.push({ label: qsTr("When %1's limit resets (%2)").arg(agentName).arg(time(new Date(limitResets))),
                        run: () => snooze.pick(snooze.limitResets) })
        return list
    }
}
