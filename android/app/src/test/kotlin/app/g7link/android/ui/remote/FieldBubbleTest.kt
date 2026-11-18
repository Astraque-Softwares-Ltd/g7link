// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Test

class FieldBubbleTest {
    @Test
    fun aLabelThatIsTheAppsNameIsDropped() {
        assertEquals(
            "the icon alone",
            BubbleNames(null, null),
            bubbleNames("com.anthropic.Claude", "Claude", window = true, iconShown = true),
        )
        assertEquals(
            "the app's name alone",
            BubbleNames("Claude", null),
            bubbleNames("com.anthropic.Claude", "Claude", window = true, iconShown = false),
        )
        assertEquals(
            "whatever the case",
            BubbleNames("Firefox", null),
            bubbleNames("firefox", "FIREFOX", window = false, iconShown = false),
        )
    }

    @Test
    fun aWindowsTitleLosesTheAppsName() {
        assertEquals(BubbleNames(null, "New Tab"), bubbleNames("google-chrome", "New Tab - Google Chrome", true, true))
        assertEquals(BubbleNames(null, "Inbox"), bubbleNames("Thunderbird", "Inbox – Thunderbird", true, true))
        assertEquals(BubbleNames(null, "notes.txt"), bubbleNames("Kate", "notes.txt — Kate", true, true))
        assertEquals("at its start", BubbleNames(null, "general"), bubbleNames("Slack", "Slack | general", true, true))
        assertEquals(
            "and the app's name before it without its icon",
            BubbleNames("Google Chrome", "New Tab"),
            bubbleNames("google-chrome", "New Tab - Google Chrome", window = true, iconShown = false),
        )
        assertEquals(
            "nothing left but the name",
            BubbleNames(null, null),
            bubbleNames("com.anthropic.Claude", "Claude - Claude", window = true, iconShown = true),
        )
    }

    @Test
    fun onlyAWindowsTitleIsCut() {
        assertEquals(
            "a field's own label stays whole",
            BubbleNames(null, "Search - Firefox"),
            bubbleNames("firefox", "Search - Firefox", window = false, iconShown = true),
        )
        assertEquals(
            "the name only where the title joins it",
            BubbleNames(null, "Google Chrome tips"),
            bubbleNames("google-chrome", "Google Chrome tips", window = true, iconShown = true),
        )
    }

    @Test
    fun aLongTitleKeepsAllButTheAppsName() {
        assertEquals(
            BubbleNames(null, "Release notes - version 2 - draft"),
            bubbleNames("google-chrome", "Release notes - version 2 - draft - Google Chrome", true, true),
        )
    }

    @Test
    fun blankOrMissingNamesAreLeftOut() {
        assertEquals(BubbleNames("Firefox", null), bubbleNames("firefox", "   ", window = true, iconShown = false))
        assertEquals(BubbleNames(null, "Search"), bubbleNames(null, "Search", window = false, iconShown = false))
        assertEquals(BubbleNames(null, "Search"), bubbleNames(" ", "Search", window = true, iconShown = false))
        assertEquals(BubbleNames(null, null), bubbleNames(null, null, window = false, iconShown = false))
    }
}
