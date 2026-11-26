// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentAskOutcome
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentMarkKind
import app.g7link.core.AgentMode
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The typing guard's rules in Kotlin (the render crate's `guard.rs`, ported from
 * t3code's `composerTypingGuard.ts`), standing in for the FFI's in JVM tests.
 */
class FakeTypingGuard : TypingGuardPort {
    private val seen = HashSet<String>()
    private val held = ArrayList<String>()
    private var focused = false
    private var lastTyped: Long? = null

    override fun requests(ids: List<String>, nowMs: Long) {
        held.retainAll(ids.toSet())
        val typing = focused && lastTyped?.let { nowMs - it < IDLE_MS } == true
        for (id in ids) {
            if (!seen.add(id)) continue
            if (typing && id !in held) held += id
        }
    }

    override fun typed(nowMs: Long) {
        lastTyped = nowMs
    }

    override fun focus() {
        focused = true
    }

    override fun blur() {
        release()
        focused = false
    }

    override fun sent() = release()

    override fun idle() = release()

    override fun held(id: String): Boolean = id in held

    override fun idleAtMs(): Long? = lastTyped?.takeIf { held.isNotEmpty() }?.let { it + IDLE_MS }

    private fun release() {
        held.clear()
        lastTyped = null
    }

    companion object {
        const val IDLE_MS = 1_500L
    }
}

class AgentDockTest {
    private val pc = "pc1"

    private fun ask(token: String, expiresMs: Long, threadId: String = "t1", truncated: Boolean = false) = AgentAsk(
        token = token,
        thread = threadId,
        threadTitle = "Fix the test",
        agent = "claude",
        kind = AgentAskKind.COMMAND,
        title = "Run command",
        command = "cargo test $token",
        diff = null,
        path = null,
        reason = null,
        truncated = truncated,
        expiresMs = expiresMs,
        item = "tool-$token",
    )

    private fun state(vararg asks: AgentAsk): AgentsState {
        var state = AgentsState(catalog = AgentCatalog(agents = emptyList(), folders = emptyList(), canSayWhy = true))
        state = AgentsModel.withThread(
            state,
            AgentThread(
                id = "t1",
                agent = "claude",
                folder = "g7linux-phone",
                title = "Fix the test",
                state = AgentThreadState.WAITING,
                mode = AgentMode.ASK,
                fromPhone = true,
                updatedMs = 1L,
                asks = asks.size.toUInt(),
                summary = null,
                error = null,
            ),
        )
        for (each in asks) state = AgentsModel.reduce(state, Event.AgentAsk(pc, each)).state
        return state
    }

    private fun queue(token: String = "a1", held: Boolean = false, truncated: Boolean = false) =
        AskQueue(shown = ask(token, 10_000L, truncated = truncated), more = 0, held = held)

    // ---- One at a time (contract §P8.1) ----

    @Test
    fun `requests are shown oldest first, the first one not held, with a count`() {
        val state = state(ask("a3", 30_000L), ask("a1", 10_000L), ask("a2", 20_000L))
        val all = state.askQueue("t1")!!
        assertEquals("a1", all.shown.token)
        assertEquals(Triple(1, 3, 2), Triple(all.position, all.total, all.more))

        // The oldest is held: the next one is shown, "2 of 3".
        val skipHeld = state.askQueue("t1") { it == "a1" }!!
        assertEquals("a2", skipHeld.shown.token)
        assertEquals(2 to 3, skipHeld.position to skipHeld.total)
        assertFalse(skipHeld.held)

        // All held: the first, held.
        val allHeld = state.askQueue("t1") { true }!!
        assertEquals("a1", allHeld.shown.token)
        assertTrue(allHeld.held)
    }

    @Test
    fun `a request that comes while you type is held until you pause, then shown`() {
        val guard = FakeTypingGuard()
        var state = state(ask("a1", 10_000L))
        guard.focus()
        guard.requests(state.asksFor("t1").map { it.token }, nowMs = 1_000L)
        assertFalse("there before typing", guard.held("a1"))

        guard.typed(nowMs = 5_000L)
        state = AgentsModel.reduce(state, Event.AgentAsk(pc, ask("a2", 20_000L))).state
        guard.requests(state.asksFor("t1").map { it.token }, nowMs = 5_400L)
        assertTrue(guard.held("a2"))
        assertEquals(6_500L, guard.idleAtMs())
        val queue = state.askQueue("t1") { guard.held(it) }!!
        assertEquals("a1", queue.shown.token)

        // a1 answered elsewhere: a2 is shown, held, so its buttons wait.
        state = AgentsModel.reduce(state, Event.AgentAsked(pc, "a1", "t1", AgentAskOutcome.ALLOWED, "Pixel")).state
        guard.requests(state.asksFor("t1").map { it.token }, nowMs = 5_600L)
        val held = state.askQueue("t1") { guard.held(it) }!!
        assertEquals("a2", held.shown.token)
        assertTrue(held.held)
        assertFalse(AgentDock.canAllow(held, answering = false))
        assertFalse(AgentDock.canDeny(held, answering = false))
        assertEquals(AgentMarkKind.HELD, AgentsModel.marks(state, "t1") { guard.held(it) }.single().kind)

        // The pause: everything goes free.
        guard.idle()
        assertNull(guard.idleAtMs())
        val free = state.askQueue("t1") { guard.held(it) }!!
        assertFalse(free.held)
        assertTrue(AgentDock.canAllow(free, answering = false))
    }

    @Test
    fun `typing that paused before a request came doesn't hold it, nor does an unfocused box`() {
        val guard = FakeTypingGuard()
        guard.focus()
        guard.typed(nowMs = 1_000L)
        guard.requests(listOf("a1"), nowMs = 2_600L)
        assertFalse(guard.held("a1"))

        val unfocused = FakeTypingGuard()
        unfocused.typed(nowMs = 1_000L)
        unfocused.requests(listOf("a1"), nowMs = 1_100L)
        assertFalse(unfocused.held("a1"))

        // Sending releases what was held.
        val sent = FakeTypingGuard()
        sent.focus()
        sent.typed(nowMs = 1_000L)
        sent.requests(listOf("a1"), nowMs = 1_100L)
        assertTrue(sent.held("a1"))
        sent.sent()
        assertFalse(sent.held("a1"))
    }

    // ---- The reply box (contract §P8.3, reconcile file "Approvals dock") ----

    @Test
    fun `with a request shown, the box is open to deny it with words when the PC takes them`() {
        val view = AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true))
        assertTrue(view.enabled)
        assertEquals(ComposerHint.SAY_WHY, view.hint)
        assertTrue("Stop stays in reach while it waits", view.showStop)
        // Empty: nothing to send.
        assertEquals(ComposerAction.SEND, view.action)
        assertFalse(view.actionEnabled)
        assertTrue(view.allowPrimary)

        val typed = AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, draft = "Use the runner", draftFor = "a1"))
        assertEquals(ComposerAction.DENY_AND_SEND, typed.action)
        assertTrue(typed.actionEnabled)
        // Words in the box: Allow is no longer the primary (no habit of "send" can allow).
        assertFalse(typed.allowPrimary)
    }

    @Test
    fun `without the PC's say-why the box stays closed while it works, and offers no denial with words`() {
        val view = AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = false, draft = "Use the runner", draftFor = "a1"))
        assertFalse(view.enabled)
        assertEquals(ComposerHint.REPLY_WHEN_STOPS, view.hint)
        assertEquals(ComposerAction.NONE, view.action)
        assertFalse(view.actionEnabled)
        assertTrue(view.showStop)
    }

    @Test
    fun `words typed under one request don't deny the next until edited, nor a held one`() {
        val changed = AgentDock.composer(ComposerInput(running = true, queue = queue("a2"), canSayWhy = true, draft = "Use the runner", draftFor = "a1"))
        assertEquals(ComposerAction.DENY_AND_SEND, changed.action)
        assertFalse(changed.actionEnabled)
        // Edited under the request now shown.
        val edited = AgentDock.editedUnder(queue("a2"))
        assertEquals("a2", edited)
        assertTrue(AgentDock.composer(ComposerInput(running = true, queue = queue("a2"), canSayWhy = true, draft = "Use the runner", draftFor = edited)).actionEnabled)

        val held = AgentDock.composer(ComposerInput(running = true, queue = queue("a1", held = true), canSayWhy = true, draft = "Use the runner", draftFor = "a1"))
        assertFalse(held.actionEnabled)
        // Nor while an answer or the words are on their way, nor when too long.
        assertFalse(AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, draft = "x", draftFor = "a1", answering = true)).actionEnabled)
        assertFalse(AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, draft = "x", draftFor = "a1", sending = true)).actionEnabled)
        assertFalse(AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, draft = "a".repeat(8001), draftFor = "a1")).actionEnabled)
    }

    @Test
    fun `Codex says it stops and reads the words next, only while they'd be sent`() {
        assertTrue(AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, draft = "x", draftFor = "a1", codex = true)).codexNote)
        assertFalse(AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, codex = true)).codexNote)
        assertFalse(AgentDock.composer(ComposerInput(running = true, queue = queue(), canSayWhy = true, draft = "x", draftFor = "a1")).codexNote)
    }

    @Test
    fun `stopped threads send messages, a paused one waits for its reset`() {
        val done = AgentDock.composer(ComposerInput(running = false, draft = "Next"))
        assertEquals(ComposerHint.MESSAGE, done.hint)
        assertEquals(ComposerAction.SEND, done.action)
        assertTrue(done.actionEnabled)
        assertFalse(done.showStop)

        val until = AgentDock.composer(ComposerInput(running = false, paused = true, pausedUntilMs = 99L, draft = "Next"))
        assertEquals(ComposerHint.AFTER_TIME, until.hint)
        assertFalse(until.enabled)
        assertFalse(until.actionEnabled)

        val unknown = AgentDock.composer(ComposerInput(running = false, paused = true, draft = "Next"))
        assertEquals(ComposerHint.AFTER_RESET, unknown.hint)
        assertTrue(unknown.enabled)

        // Working with no request: Stop only.
        val working = AgentDock.composer(ComposerInput(running = true))
        assertEquals(ComposerAction.NONE, working.action)
        assertTrue(working.showStop)
        assertFalse(working.enabled)
    }

    @Test
    fun `Allow is on at once, off while held or answering, never for one the PC cut`() {
        assertTrue("no scrolling to the end first", AgentDock.canAllow(queue(), answering = false))
        assertFalse(AgentDock.canAllow(queue(held = true), answering = false))
        assertFalse(AgentDock.canAllow(queue(), answering = true))
        assertFalse(AgentDock.canAllow(queue(truncated = true), answering = false))
        assertTrue(AgentDock.canDeny(queue(truncated = true), answering = false))
        assertFalse(AgentDock.canDeny(queue(held = true), answering = false))
    }

    @Test
    fun `Deny says it stops the turn only for a request whose denial does, and is still offered`() {
        assertFalse(AgentDock.denyStops(queue()))
        val stops = AskQueue(shown = ask("a1", 10_000L).copy(agent = "codex", denyStops = true), more = 0)
        assertTrue(AgentDock.denyStops(stops))
        assertTrue(AgentDock.canDeny(stops, answering = false))
        assertTrue(AgentDock.canAllow(stops, answering = false))
    }
}
