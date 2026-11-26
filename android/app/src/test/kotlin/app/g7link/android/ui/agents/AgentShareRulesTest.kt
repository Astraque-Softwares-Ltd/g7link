// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentMode
import app.g7link.core.AgentReadonly
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Sharing from another app to an agent (contract §V6). */
class AgentShareRulesTest {
    @Test
    fun `shared text goes in the box joined by a blank line, cut to what a message takes`() {
        assertEquals(
            SharedDraft("Daraja API: C2B callback errors\n\nhttps://developer.example/docs/c2b-errors"),
            AgentShareRules.text("Daraja API: C2B callback errors", "https://developer.example/docs/c2b-errors"),
        )
        assertEquals(SharedDraft("just text"), AgentShareRules.text(null, "  just text \n"))
        assertNull(AgentShareRules.text(" ", null))

        val long = AgentShareRules.text(null, "é".repeat(5000))!!
        assertTrue(long.shortened)
        assertTrue(AgentsModel.utf8Length(long.text) <= AgentsModel.MAX_PROMPT_BYTES)
        assertTrue(AgentsModel.fits(long.text))
        assertEquals("é".repeat(AgentsModel.MAX_PROMPT_BYTES / 2), long.text)
        // Within both limits: whole.
        val fits = "x".repeat(AgentsModel.MAX_PROMPT_CHARS)
        assertEquals(SharedDraft(fits), AgentShareRules.text(null, fits))
        val chars = AgentShareRules.text(null, "x".repeat(AgentsModel.MAX_PROMPT_CHARS + 1))!!
        assertTrue(chars.shortened)
        assertEquals(AgentsModel.MAX_PROMPT_CHARS, chars.text.length)
    }

    @Test
    fun `four pictures go, a fifth and anything that isn't one are refused by name`() {
        val ok = (1..5).map { SharedPicture("IMG_$it.jpg", null) }
        val five = AgentShareRules.pictures(ok)
        assertEquals(listOf(0, 1, 2, 3), five.added)
        assertEquals(listOf("IMG_5.jpg" to ShareRefusal.TOO_MANY), five.refused)

        val mixed = AgentShareRules.pictures(
            listOf(
                SharedPicture("report.pdf", ImageProblem.NOT_AN_IMAGE),
                SharedPicture("IMG_1.jpg", null),
                SharedPicture("huge.png", ImageProblem.TOO_LARGE),
            ),
        )
        assertEquals(listOf(1), mixed.added)
        assertEquals(listOf("report.pdf" to ShareRefusal.NOT_A_PICTURE, "huge.png" to ShareRefusal.TOO_LARGE), mixed.refused)

        // The share sheet offers it only for text and the four picture types.
        assertTrue(AgentShareRules.takes("text/plain", multiple = false))
        assertTrue(AgentShareRules.takes("image/webp", multiple = false))
        assertTrue(AgentShareRules.takes("image/*", multiple = true))
        assertFalse(AgentShareRules.takes("application/pdf", multiple = false))
        assertFalse(AgentShareRules.takes("image/heic", multiple = false))
        assertFalse(AgentShareRules.takes("video/mp4", multiple = true))
        assertFalse(AgentShareRules.takes(null, multiple = false))
    }

    private fun thread(id: String, updatedMs: Long, readonly: AgentReadonly? = null, archived: Boolean = false) = AgentThread(
        id = id,
        agent = "claude",
        folder = "duka-pos",
        title = "Thread $id",
        state = AgentThreadState.DONE,
        mode = AgentMode.ASK,
        fromPhone = false,
        updatedMs = updatedMs,
        asks = 0u,
        summary = null,
        error = null,
        archived = archived,
        readonly = readonly,
    )

    @Test
    fun `a share is added to one of the 5 newest threads that can take a message`() {
        val threads = (1..7).map { thread("t$it", updatedMs = it * 1_000L) } +
            thread("archived", updatedMs = 99_000L, archived = true)
        val state = AgentsModel.withThreads(AgentsState(), threads)
        assertEquals(listOf("t7", "t6", "t5", "t4", "t3"), AgentShareRules.threads(state).map { it.id })
    }

    @Test
    fun `a share hands its text over once, to the screen it was sent to`() {
        AgentShared.put("pc1", "t1", SharedDraft("hello"))
        assertNull(AgentShared.take("pc1", "t2"))
        assertNull(AgentShared.take("pc1", null))
        assertEquals(SharedDraft("hello"), AgentShared.take("pc1", "t1"))
        assertNull(AgentShared.take("pc1", "t1"))
        AgentShared.put("pc1", null, SharedDraft("new"))
        assertEquals(SharedDraft("new"), AgentShared.take("pc1", null))
    }

    /** Nothing in the share path sends: the activity only fills a box, and the user sends from there. */
    @Test
    fun `the share sheet has no way to send, start or answer`() {
        val source = listOf(
            File("src/main/kotlin/app/g7link/android/ui/agents/AgentShareActivity.kt"),
            File("app/src/main/kotlin/app/g7link/android/ui/agents/AgentShareActivity.kt"),
            File("android/app/src/main/kotlin/app/g7link/android/ui/agents/AgentShareActivity.kt"),
        ).firstOrNull { it.isFile } ?: error("Could not locate AgentShareActivity.kt from ${File(".").absolutePath}")
        val text = source.readText()
        for (call in listOf("agentSend(", "agentStart(", "agentAnswer(", "agentReply(", "agentImplement(", "agentQueue")) {
            assertFalse("the share sheet calls $call", text.contains(call))
        }
    }
}
