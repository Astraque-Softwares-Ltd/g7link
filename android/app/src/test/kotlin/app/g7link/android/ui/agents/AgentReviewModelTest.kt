// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** Review then commit on the phone (contract §SC6.3, §SC9.3): every ticked file read whole before Commit. */
class AgentReviewModelTest {
    private fun file(path: String, change: FileChange = FileChange.MODIFIED) = GitFile(path = path, change = change, adds = 1, dels = 1)

    private fun status(vararg paths: String, fingerprint: String = "f1") =
        GitStatus(repo = RepoKind.OK, branch = "feat/sync-retry", files = paths.map { file(it) }, total = paths.size, fingerprint = fingerprint)

    private val four = status("src/sync/queue.rs", "src/sync/backoff.rs", "src/sync/mod.rs", "tests/sync_reconnect.rs")

    /** Opened, reviewed by the PC with nothing unreadable. */
    private fun reviewed(state: ReviewState = AgentReview.open(four), id: String = "r1"): ReviewState =
        AgentReview.reviewed(state, state.tickedPaths, id, emptyMap())

    /** Reads [path] whole: the PC served it, then it was scrolled to its end. */
    private fun readWhole(state: ReviewState, path: String, patch: String = "diff of $path"): ReviewState {
        val opened = AgentReview.read(state, path)
        val served = AgentReview.served(opened, opened.review!!, path, AgentReview.hash(patch), whole = true)
        return AgentReview.reachedEnd(served, path)
    }

    private fun readAll(state: ReviewState): ReviewState = state.tickedPaths.fold(state) { s, p -> readWhole(s, p) }

    @Test
    fun `every changed file starts ticked and not read`() {
        val state = AgentReview.open(four)
        assertEquals(4, state.ticked.size)
        assertTrue(state.files.all { it.mark == ReadMark.NOT_READ })
        assertEquals(0, state.readCount)
        assertEquals(ReviewStep.FILES, state.step)
        assertTrue("a review is asked for first", AgentReview.needsReview(state))
    }

    @Test
    fun `a file is read only at its end, and only once the PC sent all of it`() {
        var state = AgentReview.startReading(reviewed())
        val path = "src/sync/queue.rs"
        assertEquals(path, state.current?.path)
        assertEquals(ReadMark.READING, state.current?.mark)
        // At its end before its bytes arrived: not read.
        assertSame(state, AgentReview.reachedEnd(state, path))
        state = AgentReview.served(state, "r1", path, AgentReview.hash("diff"), whole = true)
        assertEquals(ReadMark.READING, state.current?.mark)
        assertFalse("Next file waits for the end", AgentReview.canNext(state))
        state = AgentReview.reachedEnd(state, path)
        assertEquals(ReadMark.READ, state.current?.mark)
        assertTrue(AgentReview.canNext(state))
        assertEquals(1, state.readCount)
    }

    @Test
    fun `bytes served for another review don't count`() {
        val state = AgentReview.startReading(reviewed())
        assertSame(state, AgentReview.served(state, "r0", "src/sync/queue.rs", "h", whole = true))
    }

    @Test
    fun `Next file goes to the next unread one, then to the message`() {
        var state = AgentReview.startReading(reviewed())
        for (expected in four.files.map { it.path }) {
            assertEquals(expected, state.current?.path)
            state = AgentReview.served(state, "r1", expected, AgentReview.hash(expected), whole = true)
            state = AgentReview.reachedEnd(state, expected)
            state = AgentReview.next(state)
        }
        assertEquals(ReviewStep.MESSAGE, state.step)
        assertEquals(4, state.readCount)
    }

    @Test
    fun `back and forth is free, and a file read stays read`() {
        var state = readWhole(reviewed(), "src/sync/queue.rs")
        state = AgentReview.read(state, "src/sync/backoff.rs")
        state = AgentReview.previous(state)
        assertEquals("src/sync/queue.rs", state.current?.path)
        assertEquals(ReadMark.READ, state.current?.mark)
        assertEquals(ReviewStep.FILES, AgentReview.previous(state).step)
    }

    @Test
    fun `Commit is unreachable until every ticked file is read`() {
        var state = reviewed()
        val paths = state.tickedPaths
        for ((i, path) in paths.withIndex()) {
            state = AgentReview.toMessage(state)
            assertFalse("with $i of ${paths.size} read", AgentReview.canCommit(state, "Keep sync rows"))
            assertNull(AgentReview.commitReview(state, "Keep sync rows"))
            state = readWhole(state, path)
        }
        state = AgentReview.toMessage(state)
        assertEquals(ReviewStep.MESSAGE, state.step)
        assertTrue(AgentReview.canCommit(state, "Keep sync rows"))
        assertEquals("r1", AgentReview.commitReview(state, "Keep sync rows"))
        assertFalse("not without a message", AgentReview.canCommit(state, "   "))
    }

    @Test
    fun `an unreadable file is unticked, can't be ticked, and the review is asked for again`() {
        var state = AgentReview.open(four)
        state = AgentReview.reviewed(state, state.tickedPaths, "r1", mapOf("src/sync/mod.rs" to Unreadable.LARGE))
        val large = state.files.first { it.path == "src/sync/mod.rs" }
        assertFalse(large.ticked)
        assertEquals(Unreadable.LARGE, large.unreadable)
        assertSame(state, AgentReview.toggle(state, "src/sync/mod.rs"))
        assertTrue("the review covered it, so another is needed", AgentReview.needsReview(state))
        state = AgentReview.reviewed(state, state.tickedPaths, "r2", emptyMap())
        assertFalse(AgentReview.needsReview(state))
        assertEquals("r2", state.review)
        assertEquals(3, state.tickedPaths.size)
    }

    @Test
    fun `a file the PC couldn't send whole leaves the commit`() {
        var state = AgentReview.read(reviewed(), "src/sync/mod.rs")
        state = AgentReview.served(state, "r1", "src/sync/mod.rs", "h", whole = false)
        val file = state.files.first { it.path == "src/sync/mod.rs" }
        assertEquals(Unreadable.LARGE, file.unreadable)
        assertFalse(file.ticked)
        assertTrue(AgentReview.needsReview(state))
        assertEquals(ReviewStep.FILES, state.step)
    }

    @Test
    fun `an answer for other ticks is dropped`() {
        val state = AgentReview.toggle(AgentReview.open(four), "src/sync/mod.rs")
        assertSame(state, AgentReview.reviewed(state, four.files.map { it.path }, "r1", emptyMap()))
    }

    @Test
    fun `unticking after reading asks for a new review, and files read are checked against it`() {
        var state = readAll(reviewed())
        state = AgentReview.toggle(state, "tests/sync_reconnect.rs")
        assertTrue(AgentReview.needsReview(state))
        state = AgentReview.reviewed(state, state.tickedPaths, "r2", emptyMap())
        assertEquals(listOf("src/sync/queue.rs", "src/sync/backoff.rs", "src/sync/mod.rs"), AgentReview.toServe(state))
        assertFalse("not served by r2 yet", AgentReview.allRead(state))
        // Same bytes: still read.
        for (path in AgentReview.toServe(state)) state = AgentReview.served(state, "r2", path, AgentReview.hash("diff of $path"), whole = true)
        assertTrue(AgentReview.allRead(state))
        // Different bytes for one: read again.
        var other = readAll(reviewed())
        other = AgentReview.toggle(other, "tests/sync_reconnect.rs")
        other = AgentReview.reviewed(other, other.tickedPaths, "r2", emptyMap())
        other = AgentReview.served(other, "r2", "src/sync/queue.rs", AgentReview.hash("changed"), whole = true)
        assertEquals(ReadMark.NOT_READ, other.files.first { it.path == "src/sync/queue.rs" }.mark)
    }

    @Test
    fun `a folder change resets the screen with the files to read again`() {
        val read = AgentReview.toMessage(readAll(reviewed()))
        assertSame("the same folder changes nothing", read, AgentReview.folderChanged(read, four))
        val changed = status("src/sync/queue.rs", "src/sync/backoff.rs", "src/sync/mod.rs", "tests/sync_reconnect.rs", fingerprint = "f2")
        val reset = AgentReview.folderChanged(read, changed)
        assertTrue(reset.changed)
        assertEquals(ReviewStep.FILES, reset.step)
        assertEquals(0, reset.readCount)
        assertNull(reset.review)
        assertFalse(AgentReview.canCommit(reset, "Keep sync rows"))
        assertEquals("f2", reset.fingerprint)
    }

    @Test
    fun `without a fingerprint, a change in the files is the folder changing`() {
        val bare = four.copy(fingerprint = null)
        val state = AgentReview.open(bare)
        assertFalse(AgentReview.differs(state, bare))
        assertTrue(AgentReview.differs(state, bare.copy(files = bare.files.dropLast(1))))
    }

    @Test
    fun `a phone commits at most 200 files`() {
        val many = status(*Array(205) { "f$it.txt" })
        val state = AgentReview.open(many)
        assertTrue(state.capped)
        assertEquals(AgentGit.MAX_PHONE_PATHS, state.ticked.size)
        assertSame("a 201st isn't ticked", state, AgentReview.toggle(state, "f204.txt"))
        val fewer = AgentReview.toggle(state, "f0.txt")
        assertEquals(199, fewer.ticked.size)
        assertEquals(200, AgentReview.toggle(fewer, "f204.txt").ticked.size)
    }

    @Test
    fun `nothing ticked reads nothing and commits nothing`() {
        var state = AgentReview.open(status("a.txt"))
        state = AgentReview.toggle(state, "a.txt")
        assertFalse(AgentReview.needsReview(state))
        assertSame(state, AgentReview.startReading(state))
        assertFalse(AgentReview.allRead(state))
    }
}
