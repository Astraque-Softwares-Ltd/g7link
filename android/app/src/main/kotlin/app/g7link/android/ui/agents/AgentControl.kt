// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import kotlin.math.roundToLong

// The phone's part of Agents phase 2 ("control", contract §Q5, §Q6.1, §Q6.3):
// the models and modes, the follow-up while it works, the queue, questions,
// images, context and limits. Plain Kotlin types, filled from the FFI in
// AgentControlFfi.kt only, so the rules here run in JVM tests.

/** A model an agent's CLI reports on the PC (`ModelInfo`). */
@Immutable
data class ModelOption(
    /** What the CLI takes (`--model <id>`). */
    val id: String,
    val name: String,
    val note: String? = null,
    /** The CLI's default. */
    val isDefault: Boolean = false,
    /** Its effort levels as the CLI names them; empty: no effort choice. */
    val efforts: List<String> = emptyList(),
    val defaultEffort: String? = null,
    val images: Boolean = false,
    /** A short name that follows the newest model ("opus"): listed first. */
    val short: Boolean = false,
    /** The exact model it stands for ("Opus 5"), when the CLI says; shown in place of [name]. */
    val exact: String? = null,
)

enum class LimitState { OK, WARN, REACHED }

/** A usage window as the agent's CLI last reported it. */
@Immutable
data class LimitInfo(val window: String, val usedPct: Int? = null, val resetsMs: Long? = null, val state: LimitState = LimitState.OK)

/** What an agent can do on a PC that offers control (`AgentInfo.models`, `can`, `limits`). */
@Immutable
data class AgentAbilities(
    val models: List<ModelOption> = emptyList(),
    val can: Set<String> = emptySet(),
    val limits: List<LimitInfo> = emptyList(),
    /** The PC takes control (`AgentCatalog.can_control`): an older one offers what 0.1.16 did. */
    val control: Boolean = false,
) {
    fun can(what: String): Boolean = control && what in can

    companion object {
        const val PLAN = "plan"
        const val QUESTIONS = "questions"
        const val STEER = "steer"
        const val IMAGES = "images"
        const val COMMANDS = "commands"
        const val MCP_SWITCH = "mcp_switch"
    }
}

/** How a thread asks before acting. A phone sets only [SUPERVISED] or [PLAN]. */
enum class ThreadMode { SUPERVISED, AUTO_EDIT, PLAN }

/** A message waiting for the running turn to end. */
@Immutable
data class QueuedLine(
    val id: String,
    val text: String,
    /** Cut to fit the list: the whole of it is on the PC, so it isn't edited here. */
    val truncated: Boolean = false,
    val byName: String? = null,
    val images: Int = 0,
    val createdMs: Long = 0L,
)

/** How full a thread's context is, as the CLI last reported. */
@Immutable
data class ContextUse(val used: Long, val max: Long)

/** The user's own MCP servers in a thread: counts only, never names. */
@Immutable
data class McpCount(val on: Boolean, val servers: Int, val connected: Int, val needLogin: Int)

/** A thread's control state (`AgentThread`'s phase 2 fields). */
@Immutable
data class ThreadControl(
    val mode: ThreadMode = ThreadMode.SUPERVISED,
    /** The model for its next turn; null: the CLI's default. */
    val model: String? = null,
    val effort: String? = null,
    /** The model the CLI says answered last. */
    val answeredModel: String? = null,
    val queued: List<QueuedLine> = emptyList(),
    /** The last turn didn't finish: nothing queued is sent until the user says. */
    val queueHeld: Boolean = false,
    val context: ContextUse? = null,
    val mcp: McpCount? = null,
    /** Questions waiting for an answer. */
    val asking: Int = 0,
)

// ---- Questions (contract §Q3.1, §Q4.4) ----

@Immutable
data class QuestionOption(val label: String, val note: String? = null)

/** One ask of a question: the agent's words and the options it offers. */
@Immutable
data class QuestionAsk(
    val id: String,
    val header: String = "",
    val text: String,
    val multi: Boolean = false,
    /** The user may answer in their own words ("Something else"). */
    val other: Boolean = false,
    val options: List<QuestionOption> = emptyList(),
)

/**
 * A question an agent asks. A [secret] one reaches the phone without its asks:
 * it's answered on the PC only, and the phone may only dismiss it.
 */
@Immutable
data class QuestionInfo(
    val token: String,
    val threadId: String,
    val agent: String,
    /** The `question` item it belongs to. */
    val itemId: String,
    val secret: Boolean,
    val asks: List<QuestionAsk>,
    val createdMs: Long,
    val expiresMs: Long,
)

/** The answer to one ask: the options picked (by label) and/or the user's own words. */
@Immutable
data class QuestionAnswer(val id: String, val labels: List<String>, val other: String?)

enum class QuestionOutcome { ANSWERED, DISMISSED, EXPIRED, CANCELLED }

/** A thread's questions are answered one at a time, like its requests. */
@Immutable
data class QuestionQueue(val shown: QuestionInfo, val position: Int, val total: Int, val held: Boolean)

/** What the dock shows: a request or a question, the one asked first. */
@Immutable
sealed interface DockShown {
    data class Approval(val queue: AskQueue) : DockShown

    data class Question(val queue: QuestionQueue) : DockShown
}

/** What the reply box is while a question is shown (reconcile file, "While a question waits"). */
enum class QuestionField {
    /** "Or type your own answer": the shown ask takes the user's own words. */
    OTHER,

    /** "Choose an answer above": it doesn't. */
    CHOOSE_ABOVE,

    /** "Answer it on <PC>.": a secret question; nothing is typed for it here. */
    SECRET,
}

/**
 * The answers being chosen for the question shown: which ask is on screen,
 * the options picked per ask, whether "Something else" is picked, and the
 * words typed for it (the reply box is that field). Never saved as a draft.
 */
@Immutable
data class QuestionAnswers(
    val token: String,
    val index: Int = 0,
    val picks: Map<String, Set<String>> = emptyMap(),
    val otherOn: Set<String> = emptySet(),
    val others: Map<String, String> = emptyMap(),
)

/** The rules of the question dock (contract §Q6.1 "Question", reconcile file). */
object QuestionDock {
    /** The most a user's own answer may be: the wire's 500 bytes. */
    const val MAX_OTHER_BYTES = 500

    /** Fresh answers for [question]; [current] kept while it's still the same question. */
    fun answersFor(question: QuestionInfo, current: QuestionAnswers?): QuestionAnswers =
        if (current != null && current.token == question.token) {
            current.copy(index = current.index.coerceIn(0, (question.asks.size - 1).coerceAtLeast(0)))
        } else {
            QuestionAnswers(question.token)
        }

    /** The ask on screen; null for a secret question (the phone never has its asks). */
    fun ask(question: QuestionInfo, answers: QuestionAnswers): QuestionAsk? =
        if (question.secret) null else question.asks.getOrNull(answers.index)

    /** What the reply box is while [question] is shown. */
    fun field(question: QuestionInfo, answers: QuestionAnswers): QuestionField = when {
        question.secret -> QuestionField.SECRET
        ask(question, answers)?.other == true -> QuestionField.OTHER
        else -> QuestionField.CHOOSE_ABOVE
    }

    /** The words in the reply box for the ask on screen. */
    fun otherText(question: QuestionInfo, answers: QuestionAnswers): String =
        ask(question, answers)?.let { answers.others[it.id] }.orEmpty()

    /** Picks an option: the only one of a single-choice ask, or toggled in a multi-choice one. */
    fun choose(question: QuestionInfo, answers: QuestionAnswers, label: String): QuestionAnswers {
        val ask = ask(question, answers) ?: return answers
        if (ask.options.none { it.label == label }) return answers
        val picked = answers.picks[ask.id].orEmpty()
        return if (ask.multi) {
            answers.copy(picks = answers.picks + (ask.id to if (label in picked) picked - label else picked + label))
        } else {
            // Another option leaves the words typed in place, unused.
            answers.copy(picks = answers.picks + (ask.id to setOf(label)), otherOn = answers.otherOn - ask.id)
        }
    }

    /** Picks "Something else" (toggled in a multi-choice ask). */
    fun chooseOther(question: QuestionInfo, answers: QuestionAnswers): QuestionAnswers {
        val ask = ask(question, answers)?.takeIf { it.other } ?: return answers
        return if (ask.multi) {
            answers.copy(otherOn = if (ask.id in answers.otherOn) answers.otherOn - ask.id else answers.otherOn + ask.id)
        } else {
            answers.copy(picks = answers.picks + (ask.id to emptySet()), otherOn = answers.otherOn + ask.id)
        }
    }

    /** Typing in the reply box: the words are the ask's own answer, and "Something else" is picked. */
    fun typeOther(question: QuestionInfo, answers: QuestionAnswers, text: String): QuestionAnswers {
        val ask = ask(question, answers)?.takeIf { it.other } ?: return answers
        val picked = if (ask.multi) answers.picks else answers.picks + (ask.id to emptySet())
        return answers.copy(others = answers.others + (ask.id to text), otherOn = answers.otherOn + ask.id, picks = picked)
    }

    /** Whether an ask has an answer: an option, or "Something else" with words that fit. */
    fun answered(ask: QuestionAsk, answers: QuestionAnswers): Boolean {
        val labels = answers.picks[ask.id].orEmpty().filter { label -> ask.options.any { it.label == label } }
        if (ask.other && ask.id in answers.otherOn) {
            val words = answers.others[ask.id].orEmpty()
            // "Something else" with nothing typed answers nothing, even with options checked too.
            if (words.isBlank() || AgentsModel.utf8Length(words.trim()) > MAX_OTHER_BYTES) return false
            return true
        }
        return labels.isNotEmpty()
    }

    fun isLast(question: QuestionInfo, answers: QuestionAnswers): Boolean = answers.index >= question.asks.size - 1

    /** Next is pressed only on an ask that has an answer, and only before the last. */
    fun canNext(question: QuestionInfo, answers: QuestionAnswers, held: Boolean): Boolean {
        val ask = ask(question, answers) ?: return false
        return !held && !isLast(question, answers) && answered(ask, answers)
    }

    fun canBack(answers: QuestionAnswers): Boolean = answers.index > 0

    /** "Send answers": on the last ask, every ask answered, not held. */
    fun canSend(question: QuestionInfo, answers: QuestionAnswers, held: Boolean): Boolean =
        !held && !question.secret && question.asks.isNotEmpty() && isLast(question, answers) &&
            question.asks.all { answered(it, answers) }

    fun next(question: QuestionInfo, answers: QuestionAnswers): QuestionAnswers =
        if (canNext(question, answers, held = false)) answers.copy(index = answers.index + 1) else answers

    fun back(answers: QuestionAnswers): QuestionAnswers = if (answers.index > 0) answers.copy(index = answers.index - 1) else answers

    /** The answers to send: one per ask, labels in the ask's order, the user's words when "Something else" is picked. */
    fun answersOut(question: QuestionInfo, answers: QuestionAnswers): List<QuestionAnswer>? {
        if (question.secret || !question.asks.all { answered(it, answers) }) return null
        return question.asks.map { ask ->
            val picked = answers.picks[ask.id].orEmpty()
            val other = if (ask.other && ask.id in answers.otherOn) answers.others[ask.id]?.trim()?.ifEmpty { null } else null
            QuestionAnswer(
                id = ask.id,
                labels = ask.options.map { it.label }.filter { it in picked && (ask.multi || other == null) },
                other = other,
            )
        }
    }
}

// ---- Model, effort and mode (contract §Q6.1 "Model control", §Q6.3) ----

/** What a thread runs with next, or a new thread starts with. A null model or effort is the CLI's default. */
@Immutable
data class ThreadSettings(
    val model: String? = null,
    val effort: String? = null,
    val mode: ThreadMode = ThreadMode.SUPERVISED,
    /** [model] was typed by the user rather than picked from the list. */
    val customModel: Boolean = false,
)

/**
 * The sheet's model list: the short names first, the rest under "More models".
 * A short name that reads like one already shown (the default's "Opus 5" for
 * `opus`) moves to the rest.
 */
@Immutable
data class ModelSections(val first: List<ModelOption>, val more: List<ModelOption>)

/** What `agent.configure` changes; a null field stays as it is, an empty model or effort goes back to the default. */
@Immutable
data class ConfigureChange(val model: String?, val effort: String?, val mode: ThreadMode?, val customModel: Boolean)

/** A folder's newest thread (`AgentFolderLast`): what it ran with (null: the defaults), its title and the branch it was on. */
@Immutable
data class FolderLast(
    val agent: String,
    val model: String? = null,
    val effort: String? = null,
    val title: String = "",
    val branch: String? = null,
)

/** A PC's agent folder as a new thread reads it (`AgentFolder`): by id and label, never a path. */
@Immutable
data class FolderUse(
    val id: String,
    val label: String,
    /** When a thread there last started, else when it was added; 0 when the PC doesn't say. */
    val usedMs: Long = 0L,
    val repo: Boolean = false,
    val last: FolderLast? = null,
)

/** An agent as quick picks read it: whether it can start a thread now, and the models its CLI reports. */
@Immutable
data class PickAgent(val id: String, val name: String, val ready: Boolean, val models: List<ModelOption> = emptyList())

/**
 * What a new thread runs with: exactly what a start takes, without the mode
 * (Plan is chosen per task). A null model or effort is the default; [custom]
 * is a model the agent's list doesn't hold, sent as typed.
 */
@Immutable
data class RunChoice(val agent: String, val model: String? = null, val effort: String? = null, val custom: Boolean = false) {
    /** The same agent, model and effort (how it was chosen doesn't matter). */
    fun same(other: RunChoice): Boolean = agent == other.agent && model == other.model && effort == other.effort

    /** The settings a start sends for it, with [mode]. */
    fun settings(mode: ThreadMode = ThreadMode.SUPERVISED): ThreadSettings =
        ThreadSettings(model = model, effort = effort, mode = mode, customModel = custom && model != null)
}

/** Where a quick pick comes from: its line under the label. */
enum class PickFrom {
    /** "Last used here". */
    HERE,

    /** "Used in <folder>". */
    FOLDER,

    /** "<Agent>'s default". */
    DEFAULT,

    /** "Your choice": picked under "Other agent, model or thinking…". */
    YOURS,
}

/** A quick pick: what it runs with, and where it comes from ([folder]: the label of the folder it was used in). */
@Immutable
data class QuickPick(val choice: RunChoice, val from: PickFrom, val folder: String? = null)

/** A row of Run with: a pick, and whether it's the one the thread starts with. */
@Immutable
data class RunRow(val pick: QuickPick, val selected: Boolean)

/** The rules of the model, effort and mode sheet, and of a new thread's quick picks. */
object AgentSettings {
    /** Claude's entry that stands for its default: the sheet's own "Default" row. */
    private const val DEFAULT_ID = "default"
    private const val MAX_MODEL_ID_BYTES = 64

    /** Quick picks shown at most (contract C §2.3). */
    const val MAX_PICKS = 3

    /** The modes a phone may choose (contract §Q5): never the one that edits without asking. */
    fun phoneModes(abilities: AgentAbilities): List<ThreadMode> =
        if (abilities.can(AgentAbilities.PLAN)) listOf(ThreadMode.SUPERVISED, ThreadMode.PLAN) else listOf(ThreadMode.SUPERVISED)

    /**
     * The sheet's rows after Default: the short names (or every model when
     * the CLI names none short), each shown name once, counting the Default
     * row's; later ones that read the same move to the rest, kept in the
     * CLI's order.
     */
    fun sections(models: List<ModelOption>): ModelSections {
        val listed = models.filter { it.id != DEFAULT_ID }
        val short = listed.filter { it.short }
        val wanted = short.ifEmpty { listed }
        val seen = HashSet<String>()
        defaultModel(models)?.let(::shownName)?.let(seen::add)
        val first = wanted.filter { seen.add(rowName(it)) }
        return ModelSections(first, listed.filter { it !in first })
    }

    /** The ids of rows that read like another row, the Default row included: they carry their id as a note. */
    fun sameNamed(models: List<ModelOption>): Set<String> {
        val listed = models.filter { it.id != DEFAULT_ID }
        val names = listed.map(::rowName) + listOfNotNull(defaultModel(models)?.let(::shownName))
        val counts = names.groupingBy { it }.eachCount()
        return listed.filter { (counts[rowName(it)] ?: 0) > 1 }.mapTo(HashSet()) { it.id }
    }

    /** The CLI's own default model, when it says which. */
    fun defaultModel(models: List<ModelOption>): ModelOption? = models.firstOrNull { it.isDefault }

    fun model(models: List<ModelOption>, id: String?): ModelOption? =
        if (id == null) defaultModel(models) else models.firstOrNull { it.id == id }

    /**
     * A model's name as shown (contract C §3.2): the exact model when the CLI
     * says ("Opus 5"), else its listed name; null for Claude's "Default"
     * entry when no version is known ("<Agent>'s default", never guessed).
     */
    fun shownName(option: ModelOption): String? =
        option.exact?.takeIf { it.isNotBlank() } ?: if (option.id == DEFAULT_ID) null else option.name.ifBlank { option.id }

    private fun rowName(option: ModelOption): String = shownName(option) ?: option.id

    /** The efforts the chosen model offers; none (hidden) for a typed model the list doesn't know. */
    fun efforts(models: List<ModelOption>, settings: ThreadSettings): List<String> =
        model(models, settings.model)?.efforts.orEmpty()

    /** The effort it runs with: the one chosen, else the model's default when the CLI says. */
    fun effectiveEffort(models: List<ModelOption>, settings: ThreadSettings): String? =
        settings.effort ?: model(models, settings.model)?.defaultEffort

    /** An effort as the CLI names it, first letter capitalised ("xhigh" → "Xhigh"). */
    fun effortLabel(effort: String): String = effort.replaceFirstChar { it.uppercaseChar() }

    /** The model's shown name; null for the CLI's default; a typed one as typed. */
    fun modelName(models: List<ModelOption>, id: String?): String? {
        if (id == null || id == DEFAULT_ID) return null
        return models.firstOrNull { it.id == id }?.let(::shownName) ?: id
    }

    /**
     * The shown name of what a [model] setting runs: the listed model's, a
     * typed one as typed, and for none the model that [answered] there when
     * the list has it, else the default's. Null: "<Agent>'s default", no
     * version known.
     */
    fun settingName(models: List<ModelOption>, model: String?, answered: String? = null): String? {
        if (model != null && model != DEFAULT_ID) return modelName(models, model)
        val ran = answered?.let { id -> models.firstOrNull { it.id == id } }?.let(::shownName)
        return ran ?: defaultModel(models)?.let(::shownName)
    }

    /** Whether the model takes images: unknown models count as taking them when the agent says it does. */
    fun takesImages(abilities: AgentAbilities, settings: ThreadSettings): Boolean {
        if (!abilities.can(AgentAbilities.IMAGES)) return false
        return model(abilities.models, settings.model)?.images ?: true
    }

    /** A model from the list: its effort is kept when it offers it too. */
    fun pickModel(models: List<ModelOption>, settings: ThreadSettings, id: String?): ThreadSettings {
        val chosen = if (id == DEFAULT_ID) null else id
        val efforts = model(models, chosen)?.efforts.orEmpty()
        return settings.copy(model = chosen, customModel = false, effort = settings.effort?.takeIf { it in efforts })
    }

    /** A typed model; null when it can't be one. */
    fun typeModel(settings: ThreadSettings, typed: String): ThreadSettings? {
        val id = typed.trim()
        if (!isModelId(id)) return null
        return settings.copy(model = id, customModel = true, effort = null)
    }

    fun pickEffort(models: List<ModelOption>, settings: ThreadSettings, effort: String): ThreadSettings =
        if (effort in efforts(models, settings)) settings.copy(effort = effort) else settings

    fun pickMode(abilities: AgentAbilities, settings: ThreadSettings, mode: ThreadMode): ThreadSettings =
        if (mode in phoneModes(abilities)) settings.copy(mode = mode) else settings

    /**
     * A model id the user may type (contract §Q7): 1 to 64 bytes, no control
     * characters, white space or direction marks, not starting with `-`.
     */
    fun isModelId(id: String): Boolean =
        id.isNotEmpty() && AgentsModel.utf8Length(id) <= MAX_MODEL_ID_BYTES && !id.startsWith('-') &&
            id.none { it.isISOControl() || it.isWhitespace() || it in BIDI }

    /** The settings a thread has now, as the sheet starts from. */
    fun of(control: ThreadControl): ThreadSettings = ThreadSettings(model = control.model, effort = control.effort, mode = control.mode)

    /** What to send to change [current] into [chosen]; null when nothing changed. A phone never sends the mode that edits without asking. */
    fun change(current: ThreadSettings, chosen: ThreadSettings): ConfigureChange? {
        val model = if (chosen.model != current.model) chosen.model.orEmpty() else null
        val effort = if (chosen.effort != current.effort) chosen.effort.orEmpty() else null
        val mode = chosen.mode.takeIf { it != current.mode && it != ThreadMode.AUTO_EDIT }
        if (model == null && effort == null && mode == null) return null
        return ConfigureChange(model, effort, mode, customModel = model != null && model.isNotEmpty() && chosen.customModel)
    }

    // ---- Quick picks (contract C §2; the twin of the desktop's control.rs `quick_picks`) ----

    /**
     * A model and effort as a start sends them: the list's default model is
     * no model set, and an effort the listed model doesn't take now is
     * dropped. A model the list doesn't hold keeps its effort (nothing says it
     * can't) and goes as typed.
     */
    fun normalized(agent: PickAgent, model: String?, effort: String?): RunChoice {
        val models = agent.models
        val id = model?.takeIf { it.isNotBlank() && it != DEFAULT_ID && models.firstOrNull { m -> m.id == it }?.isDefault != true }
        val listed = model(models, id)
        val kept = effort?.takeIf { word -> listed == null || word in listed.efforts }
        return RunChoice(agent.id, id, kept, custom = id != null && models.none { it.id == id })
    }

    /** What the sheet's settings run with for [agent], as a choice. */
    fun choiceOf(agent: PickAgent, settings: ThreadSettings): RunChoice {
        val choice = normalized(agent, settings.model, settings.effort)
        return choice.copy(custom = choice.model != null && (choice.custom || settings.customModel))
    }

    /**
     * The quick picks for folder [folderId], distinct by agent, model and
     * effort, at most [MAX_PICKS]: what its newest thread ran, then what other
     * folders' newest threads ran (the most recently used folder first), then
     * each ready agent's default not yet among them. Agents that can't start
     * a thread are left out.
     */
    fun quickPicks(folders: List<FolderUse>, agents: List<PickAgent>, folderId: String): List<QuickPick> {
        val here = folders.filter { it.id == folderId }.map { it to PickFrom.HERE }
        val others = folders.filter { it.id != folderId }
            .sortedWith(compareByDescending<FolderUse> { it.usedMs }.thenBy { it.id })
            .map { it to PickFrom.FOLDER }
        val picks = ArrayList<QuickPick>(MAX_PICKS)
        for ((folder, from) in here + others) {
            if (picks.size == MAX_PICKS) return picks
            val last = folder.last ?: continue
            val agent = agents.firstOrNull { it.id == last.agent && it.ready } ?: continue
            val choice = normalized(agent, last.model, last.effort)
            if (picks.none { it.choice.same(choice) }) {
                picks += QuickPick(choice, from, folder.label.takeIf { from == PickFrom.FOLDER })
            }
        }
        for (agent in agents) {
            if (picks.size == MAX_PICKS) break
            if (agent.ready && picks.none { it.choice.agent == agent.id }) picks += QuickPick(RunChoice(agent.id), PickFrom.DEFAULT)
        }
        return picks
    }

    /** The PC's agents a folder's newest thread ran with that can't start one now: a line under Run with says why. */
    fun leftOut(folders: List<FolderUse>, agents: List<PickAgent>): List<PickAgent> =
        agents.filter { agent -> !agent.ready && folders.any { it.last?.agent == agent.id } }

    /**
     * Run with's rows: the picks, the one [chosen] (else the first) selected,
     * and [chosen] as "Your choice" after them when it's none of them.
     */
    fun runRows(picks: List<QuickPick>, chosen: RunChoice?): List<RunRow> {
        val wanted = chosen ?: picks.firstOrNull()?.choice
        val rows = picks.map { RunRow(it, wanted != null && it.choice.same(wanted)) }
        return if (wanted != null && rows.none { it.selected }) rows + RunRow(QuickPick(wanted, PickFrom.YOURS), selected = true) else rows
    }

    /** What the thread starts with: the one [chosen], else the folder's first pick. */
    fun runWith(picks: List<QuickPick>, chosen: RunChoice?): RunChoice? = chosen ?: picks.firstOrNull()?.choice

    /** A pick's model as shown: its exact name; null for a default whose version isn't known ("<Agent>'s default"). */
    fun pickName(models: List<ModelOption>, choice: RunChoice): String? = settingName(models, choice.model)

    /** A pick's effort as shown: its own, else its model's default; null for none. */
    fun pickEffort(models: List<ModelOption>, choice: RunChoice): String? =
        (choice.effort ?: model(models, choice.model)?.defaultEffort)?.let(::effortLabel)

    /** A pick's model that the agent's list (when it has one) doesn't hold now. */
    fun delisted(models: List<ModelOption>, choice: RunChoice): Boolean =
        models.isNotEmpty() && choice.model != null && models.none { it.id == choice.model }

    private val BIDI = setOf('؜', '‎', '‏', '‪', '‫', '‬', '‭', '‮', '⁦', '⁧', '⁨', '⁩')
}

// ---- Sending while it works (contract §Q3.4, §Q6.1 "Follow-up key") ----

/** What a message sent while a turn runs does. */
enum class FollowUp {
    /** Read by the agent at its next step (Claude) or at once (Codex). */
    STEER,

    /** Waits for the turn to end. */
    QUEUE,
}

object AgentFollowUp {
    /** The button's: steer when the agent can, else queue; null when the PC takes neither (an older one). */
    fun primary(abilities: AgentAbilities): FollowUp? = when {
        !abilities.control -> null
        abilities.can(AgentAbilities.STEER) -> FollowUp.STEER
        else -> FollowUp.QUEUE
    }

    /** Long-press offers the other: "Queue it instead". */
    fun other(primary: FollowUp?): FollowUp? = if (primary == FollowUp.STEER) FollowUp.QUEUE else null
}

/** The queue's rules (contract §Q3.2 `queue.*`). */
object AgentQueue {
    const val MAX = 5

    /** A message cut to fit the list is edited on the PC, where all of it is. */
    fun canEdit(line: QueuedLine): Boolean = !line.truncated

    /** "Send now" steers or starts a turn, but not while a request or a question waits. */
    fun canSendNow(waiting: Boolean): Boolean = !waiting

    /** A sixth queued message is refused: the button queues only while there's room. */
    fun hasRoom(control: ThreadControl): Boolean = control.queued.size < MAX
}

// ---- Images (contract §Q4.5, §Q6.3) ----

enum class ImageKind { PNG, JPEG, GIF, WEBP }

/** Why an image wasn't added. */
enum class ImageProblem { NOT_AN_IMAGE, TOO_LARGE, FAILED }

/** An image added to the reply box: on its way to the PC, or there with its id. */
@Immutable
sealed interface PendingImage {
    /** This screen's own key for it. */
    val key: String

    data class Uploading(override val key: String) : PendingImage

    data class Ready(override val key: String, val id: String) : PendingImage
}

object AgentImages {
    /** Images a message carries. */
    const val MAX_PER_MESSAGE = 4

    /** The most one image may be, as the wire allows. */
    const val MAX_BYTES = 10 * 1024 * 1024

    /** A thumbnail from the PC is a JPEG of at most this much. */
    const val MAX_THUMB_BYTES = 150 * 1024

    /** The four types G7 sends, told by their first bytes; the name's extension is ignored. */
    fun kind(head: ByteArray): ImageKind? = when {
        head.startsWith(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A) -> ImageKind.PNG
        head.startsWith(0xFF, 0xD8, 0xFF) -> ImageKind.JPEG
        head.startsWith(0x47, 0x49, 0x46, 0x38) -> ImageKind.GIF
        head.size >= 12 && head.startsWith(0x52, 0x49, 0x46, 0x46) &&
            head[8] == 0x57.toByte() && head[9] == 0x45.toByte() && head[10] == 0x42.toByte() && head[11] == 0x50.toByte() -> ImageKind.WEBP
        else -> null
    }

    /** Why [bytes] can't be sent; null when they can. */
    fun problem(bytes: ByteArray): ImageProblem? = when {
        bytes.size > MAX_BYTES -> ImageProblem.TOO_LARGE
        kind(bytes) == null -> ImageProblem.NOT_AN_IMAGE
        else -> null
    }

    /** Whether a thumbnail's bytes look like what the PC sends: a JPEG within its bound. */
    fun isThumb(bytes: ByteArray): Boolean = bytes.size in 3..MAX_THUMB_BYTES && kind(bytes) == ImageKind.JPEG

    fun room(images: List<PendingImage>): Int = (MAX_PER_MESSAGE - images.size).coerceAtLeast(0)

    /** Adds an image on its way, while there's room. */
    fun adding(images: List<PendingImage>, key: String): List<PendingImage> =
        if (room(images) > 0 && images.none { it.key == key }) images + PendingImage.Uploading(key) else images

    /** It reached the PC: [id] goes with the message. One removed meanwhile stays removed. */
    fun uploaded(images: List<PendingImage>, key: String, id: String): List<PendingImage> =
        images.map { if (it.key == key) PendingImage.Ready(key, id) else it }

    fun removed(images: List<PendingImage>, key: String): List<PendingImage> = images.filterNot { it.key == key }

    fun uploading(images: List<PendingImage>): Boolean = images.any { it is PendingImage.Uploading }

    fun readyIds(images: List<PendingImage>): List<String> = images.mapNotNull { (it as? PendingImage.Ready)?.id }

    private fun ByteArray.startsWith(vararg bytes: Int): Boolean =
        size >= bytes.size && bytes.indices.all { this[it] == bytes[it].toByte() }
}

// ---- Context and limits (contract §Q4.8, §Q6.1) ----

/** A usage window's name as the client words it. */
enum class WindowName { FIVE_HOUR, WEEKLY, AS_GIVEN }

object AgentUsage {
    /** "Context 12% of 1M": how much is used, from 0 to 100; null when the CLI gave no maximum. */
    fun contextPercent(context: ContextUse): Int? {
        if (context.max <= 0L) return null
        val share = context.used.coerceAtLeast(0L).toDouble() / context.max.toDouble()
        return (share * 100.0).roundToLong().coerceIn(0L, 100L).toInt()
    }

    /** A token count as the context line says it: "950", "475K", "1M", "1.5M". */
    fun tokens(count: Long): String {
        val n = count.coerceAtLeast(0L)
        return when {
            n < 1_000L -> n.toString()
            n < 999_500L -> "${(n + 500L) / 1_000L}K"
            else -> {
                val tenths = (n + 50_000L) / 100_000L
                if (tenths % 10L == 0L) "${tenths / 10L}M" else "${tenths / 10L}.${tenths % 10L}M"
            }
        }
    }

    /** `"300"` and `"five_hour"` are the 5-hour window, `"10080"` and `"seven_day"` the weekly one. */
    fun windowName(window: String): WindowName = when (window.trim().lowercase()) {
        "300", "five_hour" -> WindowName.FIVE_HOUR
        "10080", "seven_day" -> WindowName.WEEKLY
        else -> WindowName.AS_GIVEN
    }

    /** The one line pinned above the reply box: the agent's window that's reached, else one almost used. */
    fun pinned(limits: List<LimitInfo>): LimitInfo? =
        limits.firstOrNull { it.state == LimitState.REACHED } ?: limits.firstOrNull { it.state == LimitState.WARN }

    /** Whether the details show the MCP line: the folder's switch is on and the agent reported servers. */
    fun mcpShown(mcp: McpCount?): Boolean = mcp != null && mcp.on && mcp.servers > 0
}
