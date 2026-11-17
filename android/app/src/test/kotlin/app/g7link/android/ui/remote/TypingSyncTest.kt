// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TypingSyncTest {
    private var now = 0L
    private val drafts = FieldDrafts { now }

    private fun sync() = TypingSync(drafts) { now }

    private fun field(
        focus: Int,
        kind: FieldKind,
        before: String = "",
        after: String = "",
        applied: Int = 0,
        field: Long = focus.toLong() * 10,
        epoch: Int = 0,
        start: Int = 0,
        anchor: Int? = null,
    ) = PcField(focus, field, epoch, applied, kind, "App", null, false, false, start, before, after, anchor)

    /** Types [text] at the end of the box, the way a keyboard commits it. */
    private fun TypingSync.type(text: String) {
        val next = box.text + text
        onLocal(BoxText(next))
    }

    private fun TypingSync.sendAndAck(): TypingOp? {
        val op = nextOp() ?: return null
        onDone(op, TypingResult.Done)
        return op
    }

    @Test
    fun typingInAFieldWithoutSharedTextGoesAsKeysAtTheCaret() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque))
        sync.type("hel")
        assertEquals(TypingOp.Edit(1, 1, null, null, 0, 0, "hel"), sync.sendAndAck())
        sync.type("lo")
        assertEquals(TypingOp.Edit(1, 2, null, null, 0, 0, "lo"), sync.sendAndAck())
        // Backspace deletes one character.
        sync.onLocal(BoxText("hell"))
        assertEquals(TypingOp.Edit(1, 3, null, null, 0, 1, ""), sync.sendAndAck())
        assertNull(sync.nextOp())
    }

    @Test
    fun aWordCorrectedInTheMiddleIsFixedWithoutRetypingTheRest() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque))
        sync.type("teh cat sat")
        sync.sendAndAck()
        // The keyboard corrects "teh"; the caret stays after it.
        sync.onLocal(BoxText("the cat sat", 3, 3))
        // Left over " cat sat" (8), delete "eh" (2), type "he".
        assertEquals(TypingOp.Edit(1, 2, null, null, -8, 2, "he"), sync.sendAndAck())
        // Typing on, at the end, comes back right from where the caret is.
        sync.onLocal(BoxText("the cat sat!", 12, 12))
        assertEquals(TypingOp.Edit(1, 3, null, null, 8, 0, "!"), sync.sendAndAck())
    }

    @Test
    fun anEmojiIsOneBackspace() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque))
        sync.type("hi 🐝")
        sync.sendAndAck()
        sync.onLocal(BoxText("hi "))
        assertEquals(TypingOp.Edit(1, 2, null, null, 0, 1, ""), sync.sendAndAck())
    }

    @Test
    fun changesMadeWhileAnEditIsOnItsWayJoinTheNext() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque))
        sync.type("a")
        val first = sync.nextOp()!!
        sync.type("b")
        sync.type("c")
        assertNull("one op at a time", sync.nextOp())
        sync.onDone(first, TypingResult.Done)
        assertEquals(TypingOp.Edit(1, 2, null, null, 0, 0, "bc"), sync.nextOp())
    }

    @Test
    fun aFailedEditGoesAgainWithTheNextChange() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque))
        sync.type("ab")
        val op = sync.nextOp()!!
        sync.onDone(op, TypingResult.Failed)
        assertNull("not retried on its own", sync.nextOp())
        sync.type("c")
        assertEquals(TypingOp.Edit(1, 2, null, null, 0, 0, "abc"), sync.nextOp())
    }

    @Test
    fun aMirroredFieldShowsThePcsTextAndEditsByRange() {
        val sync = sync()
        var shown: BoxText? = null
        sync.onReplace = { shown = it }
        sync.onState(field(2, FieldKind.Mirror, before = "hello wor", after = "ld", start = 10))
        assertEquals(BoxText("hello world", 9, 9), shown)
        assertEquals(BoxText("hello world", 9, 9), sync.box)
        // Deleting "wor" on the phone replaces code points 16..19 of the field.
        sync.onLocal(BoxText("hello ld", 6, 6))
        assertEquals(TypingOp.Edit(2, 1, 16, 19, 0, 0, ""), sync.sendAndAck())
        // The PC's echo of that changes nothing here.
        shown = null
        sync.onState(field(2, FieldKind.Mirror, before = "hello ", after = "ld", start = 10, applied = 1))
        assertNull(shown)
    }

    @Test
    fun theLaptopsOwnEditsReachTheBoxOnceBothSidesAreQuiet() {
        val sync = sync()
        sync.onState(field(2, FieldKind.Mirror, before = "draft"))
        now += 1_000
        sync.onState(field(2, FieldKind.Mirror, before = "dra"))
        assertEquals("not at once", "draft", sync.box.text)
        now += TypingSync.QUIET_MS
        assertTrue(sync.tick())
        assertEquals(BoxText("dra", 3, 3), sync.box)
    }

    @Test
    fun aStateFromBeforeTheLastEditIsIgnored() {
        val sync = sync()
        sync.onState(field(2, FieldKind.Mirror, before = "ab"))
        sync.onLocal(BoxText("abc"))
        sync.sendAndAck()
        now += 1_000
        sync.onState(field(2, FieldKind.Mirror, before = "ab", applied = 0))
        now += 1_000
        assertTrue("nothing to take", !sync.tick())
        assertEquals("abc", sync.box.text)
    }

    @Test
    fun movingTheCaretInAMirroredFieldMovesThePcs() {
        val sync = sync()
        sync.onState(field(2, FieldKind.Mirror, before = "hello", start = 4))
        sync.onLocal(BoxText("hello", 1, 3))
        assertEquals(TypingOp.Caret(2, 1, 7, 5), sync.nextOp())
    }

    @Test
    fun eachFieldKeepsItsOwnDraft() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque, field = 100))
        sync.type("for chrome")
        sync.sendAndAck()
        sync.onState(field(2, FieldKind.Opaque, field = 200))
        assertEquals("another field starts empty", "", sync.box.text)
        sync.type("for slack")
        sync.sendAndAck()
        sync.onState(field(3, FieldKind.Opaque, field = 100))
        assertEquals("back in the first field", "for chrome", sync.box.text)
        // What was typed there is still where the PC's caret is: a Backspace deletes it.
        sync.onLocal(BoxText("for chrom"))
        assertEquals(TypingOp.Edit(3, 3, null, null, 0, 1, ""), sync.sendAndAck())
    }

    @Test
    fun aDraftTheLaptopCastDoubtOnIsDropped() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Opaque, field = 100))
        sync.type("typed here")
        sync.sendAndAck()
        sync.onState(field(1, FieldKind.Opaque, field = 100, epoch = 1))
        assertEquals("", sync.box.text)
        sync.type("x")
        sync.onState(field(2, FieldKind.None, field = 0))
        sync.onState(field(3, FieldKind.Opaque, field = 100, epoch = 2))
        assertEquals("the draft was from before the doubt", "", sync.box.text)
    }

    @Test
    fun passwordsAndClicksLeaveNoDraft() {
        val sync = sync()
        sync.onState(field(1, FieldKind.Secret, field = 100))
        sync.type("hunter2")
        sync.sendAndAck()
        sync.onState(field(2, FieldKind.Opaque, field = 200))
        sync.onState(field(3, FieldKind.Secret, field = 100))
        assertEquals("", sync.box.text)

        sync.onState(field(4, FieldKind.Opaque, field = 200))
        sync.type("draft")
        sync.sendAndAck()
        sync.onPcCaretMoved()
        assertEquals("a click moved the PC's caret: start afresh", "", sync.box.text)
        sync.onState(field(5, FieldKind.None, field = 0))
        sync.onState(field(6, FieldKind.Opaque, field = 200))
        assertEquals("", sync.box.text)
    }

    @Test
    fun graphemesAndBytesAreCountedWhole() {
        assertEquals(3, TypingSync.graphemeCount("a🐝b"))
        assertEquals(-2, TypingSync.graphemesBetween("a🐝b", 3, 0))
        assertEquals("ab", TypingSync.utf8Prefix("abé", 3))
        assertEquals("abé", TypingSync.utf8Prefix("abé", 4))
        assertEquals("a surrogate pair isn't split", 2, TypingSync.commonPrefix("ab🐝", "ab🐞"))
        assertEquals(3, TypingSync.utf16Index("a🐝b", 2))
    }

    @Test
    fun draftsAreBoundedAndExpire() {
        repeat(FieldDrafts.MAX_FIELDS + 1) { drafts.put(it.toLong(), 0, "x$it") }
        assertNull("the oldest went", drafts.take(0, 0))
        assertEquals("x1", drafts.take(1, 0))
        drafts.put(50, 0, "y".repeat(FieldDrafts.MAX_CHARS + 5))
        assertEquals(FieldDrafts.MAX_CHARS, drafts.take(50, 0)?.length)
        drafts.put(60, 0, "old")
        now += FieldDrafts.TTL_MS + 1
        assertNull(drafts.take(60, 0))
    }
}
