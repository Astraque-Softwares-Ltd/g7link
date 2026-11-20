// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// The thin scroll indicator: only when there is more to see, shown while
// scrolling, and it can be dragged.
Item {
    id: root
    width: 300
    height: 300

    Flickable {
        id: view
        width: 200
        height: 200
        contentHeight: 1000
        contentWidth: 200
    }
    G7ScrollBar {
        id: bar
        flickable: view
        anchors.top: view.top
        anchors.bottom: view.bottom
        anchors.right: view.right
    }

    TestCase {
        name: "G7ScrollBar"
        when: windowShown

        function init() {
            // The pointer away from the view.
            mouseMove(root, 290, 290)
            view.contentHeight = 1000
            view.contentY = 0
        }

        function test_only_when_there_is_more_to_see() {
            verify(bar.needed)
            view.contentHeight = 150
            verify(!bar.needed)
            verify(!bar.shown)
        }

        function test_it_shows_while_scrolling_and_fades() {
            view.contentY = 300
            verify(bar.shown)
            compare(bar.thumbLength, 40, "200 of 1000: a fifth of the track")
            tryVerify(() => !bar.shown, 2000, "pointer " + bar.pointerOver + " moving " + bar.moving)
        }

        function test_taken_and_dragged_it_scrolls() {
            // Over the view, it shows; then it can be taken.
            mouseMove(view, 100, 100)
            tryVerify(() => bar.visible)
            mousePress(bar, 4, 10)
            mouseMove(bar, 4, 90)
            mouseRelease(bar, 4, 90)
            verify(view.contentY > 300, "dragged down by half the track")
        }
    }
}
