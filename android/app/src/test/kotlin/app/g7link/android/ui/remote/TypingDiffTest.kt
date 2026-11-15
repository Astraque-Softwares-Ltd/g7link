// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Test

class TypingDiffTest {
    @Test
    fun aKeyTypedIsSentAsItself() {
        assertEquals(TypingDiff(0, "a"), typingDiff("", "a"))
        assertEquals(TypingDiff(0, "%"), typingDiff("50", "50%"))
    }

    @Test
    fun symbolsInARowEachArrive() {
        assertEquals(TypingDiff(0, "%"), typingDiff("5%", "5%%"))
    }

    @Test
    fun backspaceDeletesOneCharacter() {
        assertEquals(TypingDiff(1, ""), typingDiff("hello", "hell"))
    }

    @Test
    fun anAutocorrectedWordIsDeletedAndRetyped() {
        assertEquals(TypingDiff(2, "he "), typingDiff("teh", "the "))
        // Only what differs goes: "anyt" stays, "ing" becomes "hing ".
        assertEquals(TypingDiff(3, "hing "), typingDiff("is anyting", "is anything "))
    }

    @Test
    fun deletingWordsInTheMiddleRetypesWhatFollows() {
        assertEquals(TypingDiff(12, "is fine"), typingDiff("this part is fine", "this is fine"))
    }

    @Test
    fun aPastedOrSuggestedChunkIsSentWhole() {
        assertEquals(TypingDiff(0, "world"), typingDiff("hello ", "hello world"))
    }

    @Test
    fun anEmojiIsOneCharacterToDeleteAndIsNeverSplit() {
        val grin = "😀"
        val cry = "😢"
        assertEquals(TypingDiff(1, ""), typingDiff("hi$grin", "hi"))
        // Emoji that share their first half: replace the whole emoji.
        assertEquals(TypingDiff(1, cry), typingDiff("hi$grin", "hi$cry"))
    }

    @Test
    fun noChangeSendsNothing() {
        assertEquals(TypingDiff(0, ""), typingDiff("same", "same"))
    }

    @Test
    fun enterLoadsALinkInsteadOfBreakingTheLine() {
        // Nothing typed here: a link pasted on the PC loads in the same tab.
        assertEquals(true, enterGoesAsIs(""))
        assertEquals(true, enterGoesAsIs("https://g7.astraque.io/download"))
        assertEquals(true, enterGoesAsIs("www.example.com"))
        assertEquals(true, enterGoesAsIs("github.com/owner/repo "))
        assertEquals(true, enterGoesAsIs("localhost.test:8080/app?x=1"))
        // Words, a sentence with a link in it, or a second line are text: a new line.
        assertEquals(false, enterGoesAsIs("hello"))
        assertEquals(false, enterGoesAsIs("see example.com"))
        assertEquals(false, enterGoesAsIs("example.com\nmore"))
        assertEquals(false, enterGoesAsIs("v1.2"))
    }
}
