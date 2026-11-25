// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.G7LinkException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** Browsing a PC's folders and adding one for a new thread (contract C §5.7, docs/protocol/agents.md §15). */
class AgentPlacesModelTest {
    private fun page(place: String, vararg names: String, more: Boolean = false) =
        PlacesPage(place, names.map { PlaceEntry(it, repo = it.endsWith("-repo")) }, more)

    @Test
    fun `a place is read page by page, each page asked from where the list ends`() {
        var (state, first) = AgentPlacesModel.open(PlacesState(), "Home/code")
        assertTrue(state.loading)
        assertNull(first.offset)
        state = AgentPlacesModel.listed(state, first, page("Home/code", "api-repo", "dashboard", more = true))
        assertEquals(listOf("api-repo", "dashboard"), state.entries.map { it.name })
        assertTrue(state.more && !state.loading)
        assertTrue(state.entries[0].repo)

        val (asking, second) = AgentPlacesModel.more(state)!!
        assertEquals(2, second.offset)
        assertNull("one page at a time", AgentPlacesModel.more(asking))
        // A name the first page had already isn't listed twice.
        state = AgentPlacesModel.listed(asking, second, page("Home/code", "dashboard", "orbit", more = false))
        assertEquals(listOf("api-repo", "dashboard", "orbit"), state.entries.map { it.name })
        assertFalse(state.more)
        assertNull("nothing more to ask", AgentPlacesModel.more(state))
    }

    @Test
    fun `an answer for a place no longer shown is dropped`() {
        val (asked, a) = AgentPlacesModel.open(PlacesState(), "Home/a")
        val (state, b) = AgentPlacesModel.open(asked, "Home/b")
        val late = AgentPlacesModel.listed(state, a, page("Home/a", "old"))
        assertSame(state, late)
        assertTrue(late.loading)
        val shown = AgentPlacesModel.listed(late, b, page("Home/b", "new"))
        assertEquals(listOf("new"), shown.entries.map { it.name })
        // A later page asked before the user went elsewhere is dropped too, and so is its failure.
        val (paging, next) = AgentPlacesModel.more(shown.copy(more = true))!!
        val (moved, _) = AgentPlacesModel.open(paging, "Local Disk")
        assertSame(moved, AgentPlacesModel.listed(moved, next, page("Home/b", "x")))
        assertSame(moved, AgentPlacesModel.listFailed(moved, next, PlacesFailure.BUSY))
        // An answer claiming another place than the one asked is not believed.
        val (state2, c) = AgentPlacesModel.open(moved, "Home/c")
        assertSame(state2, AgentPlacesModel.listed(state2, c, page("Home/elsewhere", "x")))
    }

    @Test
    fun `the roots are kept for Where while a folder below them is shown`() {
        var (state, roots) = AgentPlacesModel.open(PlacesState(), "")
        state = AgentPlacesModel.listed(state, roots, page("", "Home", "Local Disk"))
        assertEquals(listOf("Home", "Local Disk"), state.roots?.map { it.name })
        val (inHome, home) = AgentPlacesModel.open(state, "Home")
        state = AgentPlacesModel.listed(inHome, home, page("Home", "code"))
        assertEquals(listOf("Home", "Local Disk"), state.roots?.map { it.name })
        assertEquals(listOf("code"), state.entries.map { it.name })
    }

    @Test
    fun `while the PC asks, Approve on the PC shows, then the folder is used`() {
        val (state, token) = AgentPlacesModel.adding(PlacesState(place = "Home/code"), "Home/code/orbit")!!
        assertFalse(state.adding!!.prompted)
        assertNull("one add at a time", AgentPlacesModel.adding(state, "Home/code/other"))
        // A prompt for another place isn't this one's.
        assertSame(state, AgentPlacesModel.prompted(state, "Home/code/other"))
        val asked = AgentPlacesModel.prompted(state, "Home/code/orbit")
        assertTrue(asked.adding!!.prompted)
        val done = AgentPlacesModel.added(asked, token)
        assertNull(done.adding)
        assertNull(done.addFailure)
        // A prompt arriving after the answer changes nothing.
        assertSame(done, AgentPlacesModel.prompted(done, "Home/code/orbit"))
        // Nor does an answer to an add that isn't waiting any more.
        assertSame(done, AgentPlacesModel.added(done, token))
    }

    @Test
    fun `a refused or failed add says why, and another can be tried`() {
        val (state, token) = AgentPlacesModel.adding(PlacesState(), "Home/Downloads/clone")!!
        val refused = AgentPlacesModel.addFailed(state, token, PlacesFailure.DENIED)
        assertNull(refused.adding)
        assertEquals(PlacesFailure.DENIED, refused.addFailure)
        assertSame(refused, AgentPlacesModel.addFailed(refused, token + 7, PlacesFailure.LIMIT))
        val (again, _) = AgentPlacesModel.adding(refused, "Home/code")!!
        assertNull(again.addFailure)
        // A root itself is never added from a phone.
        assertNull(AgentPlacesModel.adding(PlacesState(), "Home"))
        assertNull(AgentPlacesModel.adding(PlacesState(), ""))
    }

    @Test
    fun `a failed listing shows its reason, and Try again asks for the same page`() {
        val (asking, first) = AgentPlacesModel.open(PlacesState(), "Local Disk")
        val failed = AgentPlacesModel.listFailed(asking, first, PlacesFailure.BUSY)
        assertEquals(PlacesFailure.BUSY, failed.failure)
        assertFalse(failed.loading)
        val (retrying, retry) = AgentPlacesModel.retry(failed)
        assertEquals("Local Disk", retry.place)
        assertNull(retry.offset)
        assertNull(retrying.failure)
        // A later page that failed is asked again from where the list ends.
        val listed = AgentPlacesModel.listed(retrying, retry, page("Local Disk", "a", "b", more = true))
        val (paging, next) = AgentPlacesModel.more(listed)!!
        val (_, again) = AgentPlacesModel.retry(AgentPlacesModel.listFailed(paging, next, PlacesFailure.TIMEOUT))
        assertEquals(2, again.offset)
    }

    @Test
    fun `the PC's refusals read as what they mean`() {
        assertEquals(PlacesFailure.DENIED, AgentPlacesFfi.failure(G7LinkException.Denied()))
        assertEquals(PlacesFailure.DENIED, AgentPlacesFfi.failure(G7LinkException.Declined()))
        assertEquals(PlacesFailure.NOT_FOUND, AgentPlacesFfi.failure(G7LinkException.NotFound()))
        assertEquals(PlacesFailure.BUSY, AgentPlacesFfi.failure(G7LinkException.Internal("BUSY a disk was slow")))
        assertEquals(PlacesFailure.LIMIT, AgentPlacesFfi.failure(G7LinkException.Internal("LIMIT 256 folders")))
        assertEquals(PlacesFailure.OTHER, AgentPlacesFfi.failure(G7LinkException.Internal("something else")))
        assertEquals(PlacesFailure.UNSUPPORTED, AgentPlacesFfi.failure(G7LinkException.Unsupported()))
        assertEquals(PlacesFailure.TIMEOUT, AgentPlacesFfi.failure(G7LinkException.Timeout()))
        assertEquals(PlacesFailure.OFFLINE, AgentPlacesFfi.failure(G7LinkException.Offline()))
    }

    @Test
    fun `places are named from a root down, never above it`() {
        assertEquals("Home", AgentPlacesModel.child("", "Home"))
        assertEquals("Home/code/orbit", AgentPlacesModel.child("Home/code", "orbit"))
        assertEquals("Home/code", AgentPlacesModel.parent("Home/code/orbit"))
        assertEquals("", AgentPlacesModel.parent("Home"))
        assertEquals(listOf("Home" to "Home", "code" to "Home/code", "orbit" to "Home/code/orbit"), AgentPlacesModel.crumbs("Home/code/orbit"))
        assertTrue(AgentPlacesModel.crumbs("").isEmpty())
        assertTrue(AgentPlacesModel.canAdd("Local Disk/projects"))
        assertFalse(AgentPlacesModel.canAdd("Local Disk"))
        assertFalse(AgentPlacesModel.canAdd("Home//code"))
    }
}
