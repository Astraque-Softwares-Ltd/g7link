// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import app.g7link.core.Notification
import app.g7link.core.NotificationChatMessage
import app.g7link.core.NotificationConversation
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

class RepostFilterTest {
    private fun chat(vararg messages: Pair<String, Long>): Notification = Notification(
        key = "0|com.whatsapp|1|chat-a",
        app = "com.whatsapp",
        appName = "WhatsApp",
        title = "Amina",
        text = messages.last().first,
        sub = null,
        `when` = conversationTime(messages.map { it.second }, postTime = 9_999_999L),
        actions = emptyList(),
        silent = false,
        icon = null,
        image = null,
        live = null,
        conversation = NotificationConversation(
            v = 1u,
            title = "Amina",
            group = false,
            avatar = null,
            messages = messages.map { (text, time) ->
                NotificationChatMessage(sender = "Amina", text = text, time = time, selfSent = false, avatar = null)
            },
        ),
        ongoing = false,
    )

    @Test
    fun aRepostWithNothingNewLooksTheSame() {
        // WhatsApp re-posts Amina's chat when someone else writes: same messages.
        assertEquals(shownFingerprint(chat("Are we on?" to 1_000L)), shownFingerprint(chat("Are we on?" to 1_000L)))
    }

    @Test
    fun aNewMessageInTheChatIsAChange() {
        assertNotEquals(
            shownFingerprint(chat("Are we on?" to 1_000L)),
            shownFingerprint(chat("Are we on?" to 1_000L, "At 6" to 2_000L)),
        )
    }

    @Test
    fun aChatKeepsItsLatestMessagesTime() {
        assertEquals(2_000L, conversationTime(listOf(1_000L, 2_000L), postTime = 9_999L))
        assertEquals(9_999L, conversationTime(emptyList(), postTime = 9_999L))
        assertEquals(9_999L, conversationTime(listOf(0L), postTime = 9_999L))
    }
}
