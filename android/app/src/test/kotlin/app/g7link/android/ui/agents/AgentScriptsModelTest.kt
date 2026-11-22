// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Scripts sheet, its confirmation, Stop and the output screen's lines (phase 6 contract §X5.2). */
class AgentScriptsModelTest {
    private val dev = Script("s0123abcd", "dev", "npm run dev")
    private val test = Script("s4567ef01", "test", "cargo test --workspace")

    private fun folder(phoneScripts: Boolean = true, scripts: List<Script> = listOf(dev, test)) =
        ScriptsFolder(id = "f1", label = "duka-pos", phoneScripts = phoneScripts, count = scripts.size, scripts = if (phoneScripts) scripts else emptyList())

    private fun run(
        id: String = "0123456789abcdef",
        script: String = dev.id,
        name: String = dev.name,
        phase: RunPhase = RunPhase.RUNNING,
        mine: Boolean = false,
        lines: List<String> = emptyList(),
        from: Long = 0L,
        dropped: Int = 0,
        exit: Int? = null,
        reason: String? = null,
        by: String? = null,
        startedMs: Long = 1_000L,
    ) = ScriptRun(
        id = id,
        threadId = "t1",
        scriptId = script,
        name = name,
        phase = phase,
        exit = exit,
        startedMs = startedMs,
        by = by,
        mine = mine,
        lines = lines,
        from = from,
        dropped = dropped,
        reason = reason,
    )

    // ---- The sheet ----

    @Test
    fun `with the switch off no script and no command is offered, and runs stay readable`() {
        val kept = listOf(run(phase = RunPhase.DONE))
        val sheet = AgentScripts.sheet(folder(phoneScripts = false), kept, offline = false, readonly = false)
        assertTrue(sheet.scripts.isEmpty())
        assertEquals(ScriptsNote.SWITCH_OFF, sheet.note)
        assertEquals(kept, sheet.runs)
        // The folder as the phone keeps it holds no command either.
        assertTrue(folder(phoneScripts = false).scripts.isEmpty())
    }

    @Test
    fun `with the switch on each script has Run, or its run and Stop when this phone may stop it`() {
        val sheet = AgentScripts.sheet(folder(), listOf(run(mine = true)), offline = false, readonly = false)
        assertNull(sheet.note)
        val (devRow, testRow) = sheet.scripts
        assertEquals("0123456789abcdef", devRow.running?.id)
        assertFalse("one run of a script at a time", devRow.canRun)
        assertTrue(devRow.canStop)
        assertTrue(testRow.canRun)
        assertNull(testRow.running)
    }

    @Test
    fun `a read-only thread runs nothing, and a phone away from the PC only reads`() {
        assertTrue(AgentScripts.sheet(folder(), emptyList(), offline = false, readonly = true).scripts.isEmpty())
        val away = AgentScripts.sheet(folder(), listOf(run(mine = true)), offline = true, readonly = false)
        assertEquals(ScriptsNote.NEEDS_PC, away.note)
        assertTrue(away.scripts.none { it.canRun || it.canStop })
        assertTrue(away.stoppable.isEmpty())
    }

    @Test
    fun `an unknown folder shows no scripts and no switch line`() {
        val sheet = AgentScripts.sheet(null, listOf(run(phase = RunPhase.DONE)), offline = false, readonly = false)
        assertTrue(sheet.scripts.isEmpty())
        assertNull(sheet.note)
        assertEquals(1, sheet.runs.size)
    }

    @Test
    fun `Stop is offered for a run this phone started, or any run of a folder whose switch is on`() {
        val mineOff = run(mine = true)
        val otherOff = run(id = "1111111111111111", mine = false, by = "Pixel")
        val off = AgentScripts.sheet(folder(phoneScripts = false), listOf(mineOff, otherOff), offline = false, readonly = false)
        assertEquals(setOf(mineOff.id), off.stoppable)
        val on = AgentScripts.sheet(folder(), listOf(mineOff, otherOff), offline = false, readonly = false)
        assertEquals(setOf(mineOff.id, otherOff.id), on.stoppable)
        assertFalse("an ended run isn't stopped", AgentScripts.canStop(run(phase = RunPhase.DONE, mine = true), folder(), offline = false))
    }

    @Test
    fun `a brief from the thread is matched to its script by name`() {
        val brief = ScriptRun(id = "2222222222222222", threadId = "t1", scriptId = "", name = "Test", phase = RunPhase.RUNNING, brief = true)
        val sheet = AgentScripts.sheet(folder(), listOf(brief), offline = false, readonly = false)
        assertEquals(brief.id, sheet.scripts[1].running?.id)
        assertTrue("any run of a folder whose switch is on", sheet.scripts[1].canStop)
        val off = AgentScripts.sheet(folder(phoneScripts = false), listOf(brief), offline = false, readonly = false)
        assertFalse("not known to be this phone's", brief.id in off.stoppable)
    }

    @Test
    fun `Scripts is in the menu once the folder has any or the thread has runs`() {
        assertFalse(AgentScripts.menuShown(null, emptyList()))
        assertFalse(AgentScripts.menuShown(folder(scripts = emptyList()), emptyList()))
        assertTrue("counted even with the switch off", AgentScripts.menuShown(ScriptsFolder("f1", "duka-pos", false, 2, emptyList()), emptyList()))
        assertTrue(AgentScripts.menuShown(null, listOf(run(phase = RunPhase.DONE))))
    }

    @Test
    fun `a thread names its folder by label, and two folders with one label name none`() {
        val a = folder()
        val b = a.copy(id = "f2", label = "web")
        assertSame(a, AgentScripts.folderOf(listOf(a, b), "duka-pos"))
        assertNull(AgentScripts.folderOf(listOf(a, b.copy(label = "duka-pos")), "duka-pos"))
        assertNull(AgentScripts.folderOf(listOf(a), "other"))
    }

    // ---- The confirmation ----

    @Test
    fun `Run on a row only opens the confirmation, and its own Run sends once`() {
        val row = ScriptRow(dev, running = null, canRun = true, canStop = false)
        var ask: RunAsk = RunAsk.None
        assertNull("nothing to send without a confirmation", AgentScriptAsk.confirm(ask).second)
        ask = AgentScriptAsk.tap(ask, row)
        assertEquals(RunAsk.Confirm(dev), ask)
        val (sending, script) = AgentScriptAsk.confirm(ask)
        assertEquals(dev, script)
        assertEquals(RunAsk.Sending(dev), sending)
        assertNull("pressed twice: sent once", AgentScriptAsk.confirm(sending).second)
        assertEquals("another row while sending", sending, AgentScriptAsk.tap(sending, row.copy(script = test)))
        assertEquals(RunAsk.None, AgentScriptAsk.done())
    }

    @Test
    fun `Cancel sends nothing, and a row that can't run opens nothing`() {
        val row = ScriptRow(dev, running = null, canRun = true, canStop = false)
        val ask = AgentScriptAsk.tap(RunAsk.None, row)
        assertEquals(RunAsk.None, AgentScriptAsk.cancel(ask))
        assertNull(AgentScriptAsk.confirm(AgentScriptAsk.cancel(ask)).second)
        assertEquals(RunAsk.None, AgentScriptAsk.tap(RunAsk.None, row.copy(canRun = false)))
    }

    // ---- What a run says ----

    @Test
    fun `a run's state in words`() {
        assertEquals(RunWord.Running, AgentScripts.word(run()))
        assertEquals(RunWord.Done, AgentScripts.word(run(phase = RunPhase.DONE, exit = 0)))
        assertEquals(RunWord.FailedExit(3), AgentScripts.word(run(phase = RunPhase.FAILED, exit = 3)))
        assertEquals(RunWord.Failed, AgentScripts.word(run(phase = RunPhase.FAILED, reason = "signal")))
        assertEquals(RunWord.NotStarted, AgentScripts.word(run(phase = RunPhase.FAILED, reason = AgentScripts.SPAWN)))
        assertEquals(RunWord.Stopped, AgentScripts.word(run(phase = RunPhase.STOPPED)))
        assertEquals("Pixel", AgentScripts.fromPhone(run(by = "Pixel")))
        assertNull("this phone's own", AgentScripts.fromPhone(run(by = "Pixel", mine = true)))
        assertNull("the PC's", AgentScripts.fromPhone(run()))
    }

    @Test
    fun `the output says it shows the last lines when it doesn't start at the first`() {
        assertFalse(AgentScripts.tailShown(run(lines = listOf("a"))))
        assertTrue(AgentScripts.tailShown(run(lines = listOf("a"), from = 120L)))
        assertTrue(AgentScripts.tailShown(run(dropped = 4)))
        val long = (0 until AgentScripts.MAX_LINES + 10).map { "line $it" }
        val kept = AgentScripts.read(null, run(lines = long))
        assertEquals(AgentScripts.MAX_LINES, kept.lines.size)
        assertTrue("this phone let some go", AgentScripts.tailShown(kept))
    }

    @Test
    fun `a refusal says why with the PC's word first`() {
        assertEquals(ScriptRefusal.BUSY, AgentScripts.refusal(AgentFailure.OTHER, "script_running"))
        assertEquals(ScriptRefusal.LIMIT, AgentScripts.refusal(AgentFailure.OTHER, "runs"))
        assertEquals(ScriptRefusal.GONE, AgentScripts.refusal(AgentFailure.NOT_FOUND, null))
        assertEquals(ScriptRefusal.SWITCH_OFF, AgentScripts.refusal(AgentFailure.DENIED, null))
        assertEquals(ScriptRefusal.SWITCH_OFF, AgentScripts.refusal(AgentFailure.OTHER, "phone_scripts_off"))
        assertEquals(ScriptRefusal.NEEDS_PC, AgentScripts.refusal(AgentFailure.OFFLINE, null))
        assertEquals(ScriptRefusal.FAILED, AgentScripts.refusal(AgentFailure.OTHER, "folder_missing"))
    }

    // ---- Lines as they arrive ----

    @Test
    fun `lines are added in order, a gap marks the run to be read, and an ended run stays ended`() {
        var kept = AgentScripts.kept(emptyMap(), run(lines = listOf("a", "b")))
        kept = AgentScripts.kept(kept, run(lines = listOf("b", "c"), from = 1L))
        assertEquals(listOf("a", "b", "c"), kept.getValue("0123456789abcdef").lines)
        kept = AgentScripts.kept(kept, run(lines = listOf("x"), from = 10L))
        val gapped = kept.getValue("0123456789abcdef")
        assertTrue(gapped.gap)
        assertEquals("what was there is kept", listOf("a", "b", "c"), gapped.lines)
        kept = AgentScripts.kept(kept, run(phase = RunPhase.DONE, exit = 0, from = 3L), read = false)
        kept = AgentScripts.kept(kept, run(phase = RunPhase.RUNNING, from = 3L))
        assertEquals(RunPhase.DONE, kept.getValue("0123456789abcdef").phase)
        // Read whole: the gap is over.
        kept = AgentScripts.kept(kept, run(phase = RunPhase.DONE, lines = (0 until 12).map { "$it" }, from = 0L), read = true)
        assertFalse(kept.getValue("0123456789abcdef").gap)
        assertEquals(12, kept.getValue("0123456789abcdef").lines.size)
    }

    @Test
    fun `a list without lines keeps the lines already here, and the oldest runs go past the cap`() {
        var kept = AgentScripts.kept(emptyMap(), run(lines = listOf("a")), read = true)
        kept = AgentScripts.kept(kept, run(lines = emptyList(), mine = true), read = true)
        assertEquals(listOf("a"), kept.getValue("0123456789abcdef").lines)
        for (i in 0 until AgentScripts.MAX_RUNS + 3) kept = AgentScripts.kept(kept, run(id = "r$i"))
        assertEquals(AgentScripts.MAX_RUNS, kept.size)
        assertFalse(kept.containsKey("0123456789abcdef"))
    }

    @Test
    fun `a thread's runs list its own first with what was heard, running first, a stale running one ends`() {
        val heard = run(id = "a", lines = listOf("ready"), mine = true)
        val stale = run(id = "b", phase = RunPhase.RUNNING)
        val unlisted = run(id = "c", phase = RunPhase.DONE, startedMs = 5_000L)
        val briefs = listOf(
            ScriptRun("b", "t1", "", "dev", RunPhase.DONE, exit = 0, brief = true),
            ScriptRun("a", "t1", "", "dev", RunPhase.RUNNING, brief = true),
        )
        val runs = AgentScripts.runsOf("t1", briefs, mapOf("a" to heard, "b" to stale, "c" to unlisted))
        assertEquals(listOf("a", "b", "c"), runs.map { it.id })
        assertEquals(listOf("ready"), runs[0].lines)
        assertEquals(RunPhase.DONE, runs[1].phase)
        assertEquals(0, runs[1].exit)
        assertEquals(RunningLine(heard, canStop = true), AgentScripts.runningLine(runs, folder(phoneScripts = false), offline = false))
        assertNull(AgentScripts.runningLine(runs.drop(1), folder(), offline = false))
        assertTrue(AgentScripts.withoutThread(mapOf("a" to heard), "t1").isEmpty())
    }
}
