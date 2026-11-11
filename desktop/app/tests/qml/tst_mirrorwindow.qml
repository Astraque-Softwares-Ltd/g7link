// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Window
import QtTest
import app.g7link

// The mirror window is the phone's picture and nothing else: it takes the
// picture's shape, and its tools fit a window of any width.
Item {
    id: root
    width: 200
    height: 200

    Component {
        id: windowComponent
        MirrorWindow {}
    }

    TestCase {
        name: "MirrorWindow"
        when: windowShown

        property var win: null

        function describe(extra) {
            return JSON.stringify([Object.assign({
                key: "phone/0", device: "phone", session: 0, name: "Pixel", title: "Pixel", app: false,
                phase: "showing", canControl: true, raised: 0, savedWidth: 0, savedHeight: 0
            }, extra || {})])
        }

        function open(extra) {
            Mirror.asked = []
            Mirror.windows = describe(extra)
            win = createTemporaryObject(windowComponent, root, { mirrorKey: "phone/0" })
            verify(win)
            tryVerify(() => Mirror.views["phone/0"] !== undefined)
            return Mirror.views["phone/0"]
        }

        function ratio() { return win.width / win.height }

        // Everything drawn in the window, to check nothing leaves it.
        function inside(item) {
            const topLeft = item.mapToItem(win.contentItem, 0, 0)
            return topLeft.x >= 0 && topLeft.y >= 0
                && topLeft.x + item.width <= win.width + 0.5 && topLeft.y + item.height <= win.height + 0.5
        }

        function cleanup() {
            Mirror.windows = "[]"
            Mirror.views = ({})
        }

        function test_the_window_is_the_picture_in_its_shape() {
            const video = open()
            video.frameSize = Qt.size(848, 1920)
            tryVerify(() => win.sized)
            fuzzyCompare(ratio(), 848 / 1920, 0.01)
            // No header, footer or bezel: the picture fills the window.
            tryCompare(video, "width", win.width)
            tryCompare(video, "height", win.height)
        }

        function test_the_window_turns_with_the_phone() {
            const video = open()
            video.frameSize = Qt.size(848, 1920)
            tryVerify(() => win.sized)
            const longSide = win.height
            video.frameSize = Qt.size(1920, 848)
            tryVerify(() => win.width > win.height, 2000)
            fuzzyCompare(ratio(), 1920 / 848, 0.02)
            compare(win.width, longSide)
        }

        function test_a_dragged_edge_keeps_its_side_and_the_shape() {
            const video = open()
            video.frameSize = Qt.size(720, 1600)
            tryVerify(() => win.sized && win.readyForResize)
            win.width = Math.round(win.width * 0.7)
            tryVerify(() => Math.abs(ratio() - 720 / 1600) < 0.01, 2000)
        }

        function test_an_app_window_tells_the_phone_its_size_and_follows_its_answer() {
            const video = open({ app: true, pkg: "com.example", title: "Example" })
            video.frameSize = Qt.size(720, 1600)
            tryVerify(() => win.sized && win.readyForResize)
            win.width = win.height
            tryVerify(() => Mirror.count("resize") > 0, 2000)
            // The phone answers with the nearest shape it has: square.
            video.frameSize = Qt.size(800, 800)
            tryVerify(() => Math.abs(ratio() - 1) < 0.01, 2000)
            // And the phone hears the size the window ended with.
            tryVerify(() => Mirror.last("resize")[2] === win.width && Mirror.last("resize")[3] === win.height, 2000)
        }

        function test_the_tools_fit_the_narrowest_window() {
            const video = open()
            video.frameSize = Qt.size(848, 1920)
            tryVerify(() => win.sized && win.readyForResize)
            win.width = win.minimumWidth
            win.height = win.minimumHeight
            const button = findChild(win.contentItem, "toolsButton")
            const menu = findChild(win.contentItem, "toolsMenu")
            verify(button && menu)
            // Nothing shows on a picture the pointer isn't moving over.
            tryVerify(() => !button.visible, 5000)
            tryVerify(() => inside(button))
            win.openTools()
            verify(menu.opened)
            verify(menu.actions.length > 5)
            const panel = findChild(menu, "panel")
            verify(panel)
            tryVerify(() => inside(panel), 1000, "the menu stays inside a %1 px wide window".arg(win.width))
            menu.close()
        }

        function test_the_phones_keys_are_in_the_tools() {
            const video = open()
            video.frameSize = Qt.size(848, 1920)
            tryVerify(() => win.sized)
            const labels = win.toolActions().map((a) => a.label)
            for (const label of ["Back (right-click)", "Home (middle-click)", "Recent apps", "Save screenshot",
                                 "Record video", "Full screen"])
                verify(labels.includes(label), label)
            const back = win.toolActions().find((a) => a.label === "Back (right-click)")
            back.run()
            compare(Mirror.last("press")[2], "back")
        }

        function test_without_control_the_hint_fits_and_the_keys_are_left_out() {
            const video = open({ canControl: false })
            video.frameSize = Qt.size(848, 1920)
            tryVerify(() => win.sized && win.readyForResize)
            win.width = win.minimumWidth
            const hint = findChild(win.contentItem, "controlHint")
            verify(hint && hint.visible)
            tryVerify(() => inside(hint))
            for (const child of [findChild(hint, "notifyPhone"), findChild(hint, "hideHint")]) {
                verify(child)
                tryVerify(() => inside(child))
            }
            verify(!win.toolActions().some((a) => a.label === "Back (right-click)"))
        }
    }
}
