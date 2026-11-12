// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import app.g7link.core.Notification

/**
 * What a mirrored notification shows, to tell a real change from a re-post.
 * Chat apps re-post their other conversations each time a message arrives
 * (WhatsApp updates its summary that way), with nothing new in them.
 */
internal fun shownFingerprint(n: Notification): Int = listOf(
    n.title,
    n.text,
    n.sub,
    n.`when`,
    n.ongoing,
    n.actions.map { it.id to it.title },
    n.image?.contentHashCode(),
    n.conversation?.messages?.map { Triple(it.sender, it.text, it.time) },
).hashCode()

/**
 * The time a notification is from: a conversation's latest message, as the
 * phone lists it, rather than when the app last posted it again.
 */
internal fun conversationTime(messageTimes: List<Long>, postTime: Long): Long =
    messageTimes.filter { it > 0L }.maxOrNull() ?: postTime
