// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentCatalog

// The new-thread flow's rules (plans/new-thread-c-contract.md §5): "Where
// first, then quick picks". Plain Kotlin, so they run in JVM tests; the
// screens are NewThreadFlow.kt and AgentPlacesScreen.kt.

/** The flow's pages: where it works, what it does, and browsing the PC for another folder. */
enum class FlowStep { WHERE, WHAT, BROWSE }

/** What the flow holds while it's open, whatever page shows; closing it lets it all go. */
@Immutable
data class NewThreadState(
    val step: FlowStep = FlowStep.WHERE,
    /** The folder chosen, by id; null until one is. */
    val folderId: String? = null,
    val prompt: String = "",
    /** What it runs with when the user chose it; null: the folder's first quick pick. */
    val chosen: RunChoice? = null,
    /** Plan first, when the agent can (else it asks before edits and commands). */
    val plan: Boolean = false,
    /** It came from a share and no folder is chosen yet: Where says to choose one. */
    val shared: Boolean = false,
)

object NewThreadFlowModel {
    /** Recent folders shown as cards before "N more recent folders". */
    const val RECENT = 4

    /**
     * Where the flow opens: a share lands on What in the folder last started
     * in ([remembered]) while that folder is still among [folders]; anything
     * else starts on Where, a plain new thread always.
     */
    fun open(shared: SharedDraft?, remembered: String?, folders: Collection<String>): NewThreadState {
        val text = shared?.text.orEmpty()
        return if (shared != null && remembered != null && remembered in folders) {
            NewThreadState(step = FlowStep.WHAT, folderId = remembered, prompt = text)
        } else {
            NewThreadState(step = FlowStep.WHERE, prompt = text, shared = shared != null)
        }
    }

    /** Back: Browse and What go to Where; Where closes the flow (null). */
    fun back(state: NewThreadState): NewThreadState? = when (state.step) {
        FlowStep.BROWSE, FlowStep.WHAT -> state.copy(step = FlowStep.WHERE)
        FlowStep.WHERE -> null
    }

    /** A folder chosen (a card, or one found or added in Browse): What, in it. Another folder starts on its own first pick. */
    fun choose(state: NewThreadState, folderId: String): NewThreadState =
        state.copy(
            step = FlowStep.WHAT,
            folderId = folderId,
            chosen = if (folderId == state.folderId) state.chosen else null,
            shared = false,
        )

    fun browse(state: NewThreadState): NewThreadState = state.copy(step = FlowStep.BROWSE)

    fun typed(state: NewThreadState, prompt: String): NewThreadState = state.copy(prompt = prompt)

    /** A pick, or what was chosen under "Other agent, model or thinking…". */
    fun pick(state: NewThreadState, choice: RunChoice): NewThreadState = state.copy(chosen = choice)

    fun plan(state: NewThreadState, on: Boolean): NewThreadState = state.copy(plan = on)

    /** The mode it starts in: Plan only when it's chosen and the agent can plan. */
    fun mode(state: NewThreadState, canPlan: Boolean): ThreadMode = if (state.plan && canPlan) ThreadMode.PLAN else ThreadMode.SUPERVISED

    /** The folder chosen went from the PC's list: back to Where to choose another, the task kept. */
    fun checked(state: NewThreadState, folders: Collection<String>): NewThreadState {
        val id = state.folderId ?: return state
        if (id in folders) return state
        return state.copy(folderId = null, chosen = null, step = if (state.step == FlowStep.WHAT) FlowStep.WHERE else state.step)
    }

    /**
     * The order of Where's cards. Opening: [first] (the folder chosen, else
     * the one last started in) leads, then the rest by last use. While open
     * ([before] is the order shown so far) the cards stay put; a folder added
     * meanwhile comes first, one gone leaves.
     */
    fun order(folders: List<FolderUse>, first: String?, before: List<String>?): List<String> {
        val ids = folders.mapTo(LinkedHashSet()) { it.id }
        val byUse = folders.sortedWith(compareByDescending<FolderUse> { it.usedMs }.thenBy { it.label.lowercase() }.thenBy { it.id }).map { it.id }
        if (before != null) {
            val shown = before.toSet()
            return byUse.filter { it !in shown } + before.filter { it in ids }
        }
        return if (first != null && first in ids) listOf(first) + byUse.filter { it != first } else byUse
    }

    /** Start: an agent that can run, a folder, a task that fits, no picture still on its way, and a model that takes them. */
    fun canStart(busy: Boolean, agentReady: Boolean, hasFolder: Boolean, prompt: String, uploading: Boolean, imagesTaken: Boolean): Boolean =
        !busy && agentReady && hasFolder && AgentsModel.fits(prompt) && !uploading && imagesTaken
}

/** The catalog as the flow reads it. */
internal object NewThreadFfi {
    fun folders(catalog: AgentCatalog): List<FolderUse> = catalog.folders.map { folder ->
        FolderUse(
            id = folder.id,
            label = folder.label,
            usedMs = if (folder.used > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else folder.used.toLong(),
            repo = folder.repo,
            last = folder.last?.let { last ->
                FolderLast(
                    agent = last.agent,
                    model = last.model?.takeIf { it.isNotBlank() },
                    effort = last.effort?.takeIf { it.isNotBlank() },
                    title = last.title,
                    branch = last.branch?.takeIf { it.isNotBlank() },
                )
            },
        )
    }

    /** Each agent of the PC, in the catalog's order: ready when installed and signed in. */
    fun agents(catalog: AgentCatalog): List<PickAgent> = catalog.agents.map { agent ->
        PickAgent(
            id = agent.id,
            name = agent.name.ifBlank { agent.id.replaceFirstChar { it.uppercaseChar() } },
            ready = agent.installed && agent.signedIn,
            models = AgentControlFfi.abilities(agent, catalog.canControl).models,
        )
    }
}
