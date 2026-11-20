// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The reply box in phase 2 (contract §Q6.1; reconcile file, "While a question waits"). */
class AgentComposerTest {
    private fun composer(input: ComposerInput) = AgentDock.composer(input)

    @Test
    fun `while a question waits there is no send path, only its own field and Stop`() {
        for (field in QuestionField.entries) {
            val view = composer(
                ComposerInput(running = true, waiting = true, question = field, draft = "Finish it", followUp = FollowUp.STEER, imagesAllowed = true),
            )
            assertEquals(field.name, ComposerAction.NONE, view.action)
            assertFalse(view.actionEnabled)
            assertTrue("Stop stays beside it", view.showStop)
            // An answer takes no image, and settings apply between turns: both inert (the boards' final check).
            assertFalse(view.attach)
            assertEquals(SettingsAccess.INERT, view.settings)
            assertNull(view.followUp)
            assertNull(view.followNote)
        }
    }

    @Test
    fun `the field is open only for an ask that takes the user's own words, never for a secret one`() {
        val other = composer(ComposerInput(running = true, waiting = true, question = QuestionField.OTHER))
        assertTrue(other.enabled)
        assertEquals(ComposerHint.OTHER_ANSWER, other.hint)
        val choose = composer(ComposerInput(running = true, waiting = true, question = QuestionField.CHOOSE_ABOVE))
        assertFalse(choose.enabled)
        assertEquals(ComposerHint.CHOOSE_ABOVE, choose.hint)
        val secret = composer(ComposerInput(running = true, waiting = true, question = QuestionField.SECRET))
        assertFalse(secret.enabled)
        assertEquals(ComposerHint.ANSWER_ON_PC, secret.hint)
    }

    @Test
    fun `while it works a message steers it, with text only, Stop beside it, the other on long-press`() {
        val empty = composer(ComposerInput(running = true, followUp = FollowUp.STEER))
        assertTrue(empty.enabled)
        assertEquals(ComposerAction.NONE, empty.action)
        assertTrue(empty.showStop)
        assertEquals(SettingsAccess.LATER, empty.settings)

        val typed = composer(ComposerInput(running = true, followUp = FollowUp.STEER, draft = "Build only arm64"))
        assertEquals(ComposerAction.FOLLOW_UP, typed.action)
        assertTrue(typed.actionEnabled)
        assertEquals(FollowUp.STEER, typed.followUp)
        assertEquals(FollowUp.QUEUE, typed.otherFollowUp)
        assertEquals(FollowUp.STEER, typed.followNote)
        assertTrue(typed.showStop)

        // A full queue (5) takes no sixth: steering stays, queueing isn't offered.
        val full = composer(ComposerInput(running = true, followUp = FollowUp.STEER, queueRoom = false, draft = "x"))
        assertEquals(FollowUp.STEER, full.followUp)
        assertNull(full.otherFollowUp)
        val queueOnly = composer(ComposerInput(running = true, followUp = FollowUp.QUEUE, queueRoom = false, draft = "x"))
        assertEquals(ComposerAction.NONE, queueOnly.action)
        assertFalse(queueOnly.enabled)
    }

    @Test
    fun `an agent that can't be steered queues, with no long-press`() {
        val view = composer(ComposerInput(running = true, followUp = FollowUp.QUEUE, draft = "Then bump versionCode"))
        assertEquals(FollowUp.QUEUE, view.followUp)
        assertNull(view.otherFollowUp)
        assertEquals(FollowUp.QUEUE, view.followNote)
    }

    @Test
    fun `nothing is sent to a turn that waits for an answer, and an older PC keeps the old box`() {
        val waiting = composer(ComposerInput(running = true, waiting = true, followUp = FollowUp.STEER, draft = "x"))
        assertEquals(ComposerAction.NONE, waiting.action)
        assertFalse(waiting.enabled)
        val older = composer(ComposerInput(running = true, draft = "x"))
        assertEquals(ComposerHint.REPLY_WHEN_STOPS, older.hint)
        assertFalse(older.enabled)
        assertFalse(older.attach)
    }

    @Test
    fun `images go with or without words, but not while one uploads or to a model that can't take them`() {
        val idle = composer(ComposerInput(running = false, images = 1, imagesAllowed = true))
        assertTrue(idle.actionEnabled)
        assertTrue(idle.attach)
        assertEquals(SettingsAccess.OPEN, idle.settings)
        assertFalse(composer(ComposerInput(running = false, images = 1, imagesAllowed = true, uploading = true)).actionEnabled)
        assertFalse(composer(ComposerInput(running = false, images = 1, imagesAllowed = false, draft = "Look")).actionEnabled)
        assertFalse(composer(ComposerInput(running = false, imagesAllowed = false)).attach)
        val steer = composer(ComposerInput(running = true, followUp = FollowUp.STEER, images = 2, imagesAllowed = true))
        assertEquals(ComposerAction.FOLLOW_UP, steer.action)
        assertTrue(steer.actionEnabled)
    }

    @Test
    fun `denying with words takes no image and keeps the model for later`() {
        val ask = AskQueue(
            shown = app.g7link.core.AgentAsk(
                token = "a1",
                thread = "t1",
                threadTitle = "",
                agent = "claude",
                kind = app.g7link.core.AgentAskKind.COMMAND,
                title = "Run command",
                command = "cargo test",
                diff = null,
                path = null,
                reason = null,
                truncated = false,
                expiresMs = 1L,
            ),
            more = 0,
        )
        val view = composer(ComposerInput(running = true, waiting = true, queue = ask, canSayWhy = true, draft = "No", draftFor = "a1", imagesAllowed = true, followUp = FollowUp.STEER))
        assertEquals(ComposerAction.DENY_AND_SEND, view.action)
        assertFalse(view.attach)
        assertEquals(SettingsAccess.LATER, view.settings)
    }
}
