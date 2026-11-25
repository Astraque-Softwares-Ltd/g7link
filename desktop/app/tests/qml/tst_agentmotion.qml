// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// The Agents chat's rows glide only while what the reader just opened or
// closed settles, as on the phone: never for rows that stream in.
TestCase {
    name: "AgentMotion"

    function init() {
        AppController.reduceMotion = false
        Agents.reset()
        AgentMotion.settle.stop()
        AgentMotion.gliding = false
        AgentMotion.waiting = false
    }
    function cleanup() {
        AppController.reduceMotion = true
    }

    function test_a_click_glides_and_still_toggles() {
        AgentMotion.toggle("g:3")
        compare(Agents.calls, ["toggle g:3"])
        verify(AgentMotion.gliding)
        verify(AgentMotion.waiting)
    }

    function test_its_rows_settle_then_it_stops() {
        AgentMotion.toggle("g:3")
        AgentMotion.landed()
        verify(AgentMotion.gliding, "while they move")
        verify(!AgentMotion.waiting)
        compare(AgentMotion.settle.interval, AgentMotion.moveMs + AgentMotion.inDelay + 120)
        tryCompare(AgentMotion, "gliding", false, 2000)
    }

    function test_later_rows_dont_keep_it_open() {
        AgentMotion.toggle("g:3")
        AgentMotion.landed()
        const settle = AgentMotion.settle.interval
        AgentMotion.landed()
        compare(AgentMotion.settle.interval, settle)
        verify(!AgentMotion.waiting)
    }

    function test_rows_without_a_click_never_glide() {
        AgentMotion.landed()
        verify(!AgentMotion.gliding)
    }

    function test_a_click_whose_rows_never_come_stops_after_its_wait() {
        AgentMotion.toggle("f:1")
        compare(AgentMotion.settle.interval, AgentMotion.wait)
        tryCompare(AgentMotion, "gliding", false, AgentMotion.wait + 1000)
    }

    function test_nothing_moves_under_reduced_motion() {
        AppController.reduceMotion = true
        AgentMotion.toggle("g:3")
        compare(Agents.calls, ["toggle g:3"])
        verify(!AgentMotion.gliding)
    }
}
