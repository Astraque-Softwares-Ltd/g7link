// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentPlaces
import app.g7link.core.G7LinkException

// Browsing a PC's folders for a new thread (docs/protocol/agents.md §15,
// contract C §5.7): the PC's roots (Home, its disks), the folders in a place
// page by page, and adding one as an agent folder while someone at the PC may
// be asked. Plain Kotlin, so the rules run in JVM tests; the FFI is read in
// AgentPlacesFfi below only.

/** One folder in a place: its name, whether it holds a repository of its own, and its id when it's already an agent folder. */
@Immutable
data class PlaceEntry(val name: String, val repo: Boolean = false, val folder: String? = null)

/** One page of a place as the PC answered it. */
@Immutable
data class PlacesPage(val place: String, val entries: List<PlaceEntry>, val more: Boolean)

/** Why listing a place or adding a folder didn't work. */
enum class PlacesFailure {
    /** This phone may not (agents off), the person at the PC said no, or the place may not be added from a phone. */
    DENIED,

    /** The place isn't there any more. */
    NOT_FOUND,

    /** A disk was slow to answer: worth trying again. */
    BUSY,

    /** The PC has as many agent folders as it keeps. */
    LIMIT,

    /** The PC doesn't offer browsing (an older one). */
    UNSUPPORTED,
    TIMEOUT,
    OFFLINE,
    OTHER,
}

/** The answer to a listing or an add. */
sealed interface PlacesAnswer<out T> {
    data class Ok<T>(val value: T) : PlacesAnswer<T>

    data class Failed(val failure: PlacesFailure) : PlacesAnswer<Nothing>
}

/** A listing to ask the PC for: answers carry [token], and only the newest one asked is shown. */
@Immutable
data class PlacesRequest(val token: Long, val place: String, val offset: Int?)

/** PC [pcId] is asking its user about adding the folder at [place] (`Event.AgentFolderPrompt`). */
@Immutable
data class FolderPrompt(val pcId: String, val place: String)

/** A folder being added: [prompted] once the PC says it's asking its user ("Approve on <PC>"). */
@Immutable
data class PlaceAdding(val place: String, val token: Long, val prompted: Boolean = false)

/** The browse page's state. */
@Immutable
data class PlacesState(
    /** The place shown; "" lists the roots. */
    val place: String = "",
    /** The place shown is already this agent folder: "Use this folder" adds nothing. */
    val folder: String? = null,
    val entries: List<PlaceEntry> = emptyList(),
    val more: Boolean = false,
    val loading: Boolean = false,
    val failure: PlacesFailure? = null,
    /** The newest listing asked for; an answer to another is stale. */
    val asked: Long = 0L,
    /** The roots as last listed, for Where and the root chips. */
    val roots: List<PlaceEntry>? = null,
    val adding: PlaceAdding? = null,
    val addFailure: PlacesFailure? = null,
    /** The last token handed out, listings and adds alike. */
    val tokens: Long = 0L,
)

object AgentPlacesModel {
    /** The most folders a place shows: the PC reads no further (its offset bound). */
    const val MAX_ENTRIES = 4_000

    /** Shows [place] ([folder]: it's already that agent folder), from its first page. */
    fun open(state: PlacesState, place: String, folder: String? = null): Pair<PlacesState, PlacesRequest> {
        val token = state.tokens + 1
        val next = state.copy(
            place = place,
            folder = folder,
            entries = emptyList(),
            more = false,
            loading = true,
            failure = null,
            asked = token,
            tokens = token,
            addFailure = null,
        )
        return next to PlacesRequest(token, place, null)
    }

    /** The next page of the place shown; null when there's none, or one is on its way. */
    fun more(state: PlacesState): Pair<PlacesState, PlacesRequest>? {
        if (!state.more || state.loading || state.entries.size >= MAX_ENTRIES) return null
        val token = state.tokens + 1
        return state.copy(loading = true, failure = null, asked = token, tokens = token) to
            PlacesRequest(token, state.place, state.entries.size)
    }

    /** Asks again after a failure: the page that failed. */
    fun retry(state: PlacesState): Pair<PlacesState, PlacesRequest> =
        if (state.entries.isEmpty()) open(state, state.place, state.folder) else more(state.copy(more = true)) ?: open(state, state.place, state.folder)

    /**
     * The PC answered [request]: the page replaces the list (the first) or
     * follows it (a later one, at the offset asked). An answer to a listing
     * no longer shown, or for another place, is dropped.
     */
    fun listed(state: PlacesState, request: PlacesRequest, page: PlacesPage): PlacesState {
        if (request.token != state.asked || page.place != request.place || request.place != state.place) return state
        val fresh = page.entries.filter { it.name.isNotEmpty() }
        val entries = if (request.offset == null || request.offset == 0) {
            fresh.distinctBy { it.name }
        } else {
            if (request.offset != state.entries.size) return state.copy(loading = false)
            val have = state.entries.mapTo(HashSet()) { it.name }
            state.entries + fresh.filter { have.add(it.name) }
        }.take(MAX_ENTRIES)
        return state.copy(
            entries = entries,
            more = page.more && entries.size < MAX_ENTRIES && fresh.isNotEmpty(),
            loading = false,
            failure = null,
            roots = if (request.place.isEmpty()) entries else state.roots,
        )
    }

    /** Listing [request] failed; dropped when it's no longer the one shown. */
    fun listFailed(state: PlacesState, request: PlacesRequest, failure: PlacesFailure): PlacesState {
        if (request.token != state.asked) return state
        return state.copy(loading = false, failure = failure, more = state.more && state.entries.isNotEmpty())
    }

    /** Starts adding the folder at [place]; null while another add waits. */
    fun adding(state: PlacesState, place: String): Pair<PlacesState, Long>? {
        if (state.adding != null || !canAdd(place)) return null
        val token = state.tokens + 1
        return state.copy(adding = PlaceAdding(place, token), addFailure = null, tokens = token) to token
    }

    /** The PC says it's asking its user about [place]: only for the add still waiting on that place. */
    fun prompted(state: PlacesState, place: String): PlacesState {
        val adding = state.adding ?: return state
        if (adding.place != place || adding.prompted) return state
        return state.copy(adding = adding.copy(prompted = true))
    }

    /** The add with [token] went through (the flow then goes on with the folder). */
    fun added(state: PlacesState, token: Long): PlacesState =
        if (state.adding?.token == token) state.copy(adding = null, addFailure = null) else state

    /** The add with [token] didn't. */
    fun addFailed(state: PlacesState, token: Long, failure: PlacesFailure): PlacesState =
        if (state.adding?.token == token) state.copy(adding = null, addFailure = failure) else state

    /** The place of the folder [name] in [place]. */
    fun child(place: String, name: String): String = if (place.isEmpty()) name else "$place/$name"

    /** The place [place] is in; "" for a root. */
    fun parent(place: String): String = place.substringBeforeLast('/', missingDelimiterValue = "")

    /** Each step from the root to [place], as (name, place). */
    fun crumbs(place: String): List<Pair<String, String>> {
        if (place.isEmpty()) return emptyList()
        val parts = place.split('/')
        return parts.indices.map { i -> parts[i] to parts.subList(0, i + 1).joinToString("/") }
    }

    /** A root itself ("Home", a disk's top) can't be added from a phone; anything below one can. */
    fun canAdd(place: String): Boolean = place.contains('/') && place.split('/').none { it.isEmpty() }
}

/** The FFI's answers as [AgentPlacesModel] reads them. */
internal object AgentPlacesFfi {
    fun page(places: AgentPlaces): PlacesPage =
        PlacesPage(places.place, places.entries.map { PlaceEntry(it.name, it.repo, it.folder?.takeIf { id -> id.isNotEmpty() }) }, places.more)

    /** What a refused listing or add means: `Internal` "BUSY…" and "LIMIT…" are the PC's own reasons. */
    fun failure(e: G7LinkException): PlacesFailure = when (e) {
        is G7LinkException.Denied, is G7LinkException.Declined -> PlacesFailure.DENIED
        is G7LinkException.NotFound -> PlacesFailure.NOT_FOUND
        is G7LinkException.Busy -> PlacesFailure.BUSY
        is G7LinkException.Unsupported -> PlacesFailure.UNSUPPORTED
        is G7LinkException.Timeout -> PlacesFailure.TIMEOUT
        is G7LinkException.Offline, is G7LinkException.NotPaired -> PlacesFailure.OFFLINE
        is G7LinkException.Internal -> when {
            e.reason.startsWith("BUSY") -> PlacesFailure.BUSY
            e.reason.startsWith("LIMIT") -> PlacesFailure.LIMIT
            else -> PlacesFailure.OTHER
        }
        else -> PlacesFailure.OTHER
    }
}
