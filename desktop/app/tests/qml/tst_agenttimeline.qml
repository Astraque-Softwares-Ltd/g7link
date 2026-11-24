// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// Every kind of row the timeline model gives has a delegate that takes all
// the roles it needs, by name.
Item {
    id: root
    width: 760
    height: 3200

    Component {
        id: timelineView
        AgentTimelineView {
            width: 720
            height: 3000
            folder: "g7linux"
            animate: false
        }
    }

    TestCase {
        name: "AgentTimeline"
        when: windowShown

        function test_every_row_has_its_delegate() {
            failOnWarning(/Required property|not initialized|TypeError|ReferenceError|Unable to assign|Cannot assign/)
            const view = createTemporaryObject(timelineView, root)
            verify(view)
            tryCompare(view, "count", AgentTimeline.count)
            for (let i = 0; i < view.count; ++i) {
                view.positionViewAtIndex(i, ListView.Contain)
                const item = view.itemAtIndex(i)
                verify(item, "row " + i + " (" + AgentTimeline.get(i).view + ") has a delegate")
                verify(item.height > 0, "row " + i + " has a height")
            }
        }

        function test_a_link_reaches_the_page_with_its_row() {
            const view = createTemporaryObject(timelineView, root)
            const spy = createTemporaryObject(spyComponent, root, { target: view, signalName: "linkActivated" })
            let block = null
            for (let i = 0; i < view.count && !block; ++i) {
                if (AgentTimeline.get(i).key === "b:m1:1")
                    block = view.itemAtIndex(i)
            }
            verify(block)
            findChild(block, "text").linkActivated("0")
            compare(spy.count, 1)
            compare(spy.signalArguments[0][0], "b:m1:1")
            compare(spy.signalArguments[0][1], 0)
        }

        // Read further up, a new row at the end turns "Jump to latest" into
        // "New below"; following again (a send, or the button) clears it.
        function test_rows_arriving_while_reading_up_say_new_below() {
            const view = createTemporaryObject(timelineView, root)
            tryCompare(view, "count", AgentTimeline.count)
            view.follow = false
            verify(!view.newBelow)
            view.checkNewBelow()
            verify(!view.newBelow, "nothing new yet")
            const last = AgentTimeline.count - 1
            const was = AgentTimeline.get(last).itemId
            AgentTimeline.setProperty(last, "itemId", "z9")
            view.checkNewBelow()
            AgentTimeline.setProperty(last, "itemId", was)
            verify(view.newBelow)
            view.jumpToLatest()
            verify(view.follow)
            verify(!view.newBelow)
        }
    }

    Component {
        id: spyComponent
        SignalSpy {}
    }
}
