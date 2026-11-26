// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentAnswer
import app.g7link.core.AgentCan
import app.g7link.core.AgentDispatch
import app.g7link.core.AgentFolderMode
import app.g7link.core.AgentInfo
import app.g7link.core.AgentItem
import app.g7link.core.AgentLimitState
import app.g7link.core.AgentMode
import app.g7link.core.AgentQuestion
import app.g7link.core.AgentQuestionAsk
import app.g7link.core.AgentQuestionOption
import app.g7link.core.AgentQuestionOutcome
import app.g7link.core.AgentThread

/**
 * The one place that reads the FFI's phase 2 records (contract §Q5, records of
 * `g7link-ffi`): the rest of Agents uses the plain types of AgentControl.kt.
 */
internal object AgentControlFfi {
    /** What [agent] can do; nothing of it unless the PC takes control ([control], `AgentCatalog.canControl`). */
    fun abilities(agent: AgentInfo?, control: Boolean): AgentAbilities {
        if (agent == null || !control) return AgentAbilities()
        return AgentAbilities(
            control = true,
            models = agent.models.map { model ->
                ModelOption(
                    id = model.id,
                    name = model.name,
                    note = model.note,
                    isDefault = model.isDefault,
                    efforts = model.efforts,
                    defaultEffort = model.defaultEffort,
                    images = model.images,
                    short = model.short,
                    exact = model.exact?.takeIf { it.isNotBlank() },
                )
            },
            can = agent.can.mapTo(HashSet()) { can ->
                when (can) {
                    AgentCan.PLAN -> AgentAbilities.PLAN
                    AgentCan.QUESTIONS -> AgentAbilities.QUESTIONS
                    AgentCan.STEER -> AgentAbilities.STEER
                    AgentCan.IMAGES -> AgentAbilities.IMAGES
                    AgentCan.MCP_SWITCH -> AgentAbilities.MCP_SWITCH
                    AgentCan.COMMANDS -> AgentAbilities.COMMANDS
                }
            },
            limits = agent.limits.map { limit ->
                LimitInfo(
                    window = limit.window,
                    usedPct = limit.usedPct?.toInt(),
                    resetsMs = limit.resetsMs,
                    state = when (limit.state) {
                        AgentLimitState.OK -> LimitState.OK
                        AgentLimitState.WARN -> LimitState.WARN
                        AgentLimitState.REACHED -> LimitState.REACHED
                    },
                )
            },
            modes = agent.modes.map(::mode).distinct(),
        )
    }

    /** A mode as the phone reads it; the FFI reads the old words too (`supervised` is Ask, `auto_edit` Accept edits). */
    fun mode(mode: AgentMode): ThreadMode = when (mode) {
        AgentMode.ASK -> ThreadMode.ASK
        AgentMode.ACCEPT_EDITS -> ThreadMode.ACCEPT_EDITS
        AgentMode.AUTO -> ThreadMode.AUTO
        AgentMode.PLAN -> ThreadMode.PLAN
        AgentMode.FULL -> ThreadMode.FULL
    }

    /** The mode a start, a change or an implement sends: any the agent offers. */
    fun wire(mode: ThreadMode): AgentMode = when (mode) {
        ThreadMode.ASK -> AgentMode.ASK
        ThreadMode.ACCEPT_EDITS -> AgentMode.ACCEPT_EDITS
        ThreadMode.AUTO -> AgentMode.AUTO
        ThreadMode.PLAN -> AgentMode.PLAN
        ThreadMode.FULL -> AgentMode.FULL
    }

    /** A folder's remembered mode per agent (`AgentFolder.modes`): the first for each agent, never Plan. */
    fun folderModes(modes: List<AgentFolderMode>): Map<String, ThreadMode> {
        val byAgent = LinkedHashMap<String, ThreadMode>()
        for (each in modes) {
            val mode = mode(each.mode)
            if (mode != ThreadMode.PLAN) byAgent.putIfAbsent(each.agent, mode)
        }
        return byAgent
    }

    fun control(thread: AgentThread): ThreadControl = ThreadControl(
        mode = mode(thread.mode),
        model = thread.model,
        effort = thread.effort,
        answeredModel = thread.answeredModel,
        queued = thread.queued.map { queued ->
            QueuedLine(
                id = queued.id,
                text = queued.text,
                truncated = queued.truncated,
                byName = queued.byName,
                images = queued.images.size,
                createdMs = queued.createdMs,
            )
        },
        queueHeld = thread.queueHeld,
        context = thread.context?.let { ContextUse(it.used.toLongClamped(), it.max.toLongClamped()) },
        mcp = thread.mcp?.let { McpCount(it.on, it.servers.toInt(), it.connected.toInt(), it.needLogin.toInt()) },
        asking = thread.asking.toInt(),
    )

    /** A question as the dock reads it; a secret one never has asks on a phone (they're dropped here too). */
    fun question(question: AgentQuestion): QuestionInfo {
        val secret = question.secret || question.asks.any { it.secret }
        return QuestionInfo(
            token = question.token,
            threadId = question.thread,
            agent = question.agent,
            itemId = question.item,
            secret = secret,
            asks = if (secret) {
                emptyList()
            } else {
                question.asks.map { ask ->
                    QuestionAsk(
                        id = ask.id,
                        header = ask.header,
                        text = ask.text,
                        multi = ask.multi,
                        other = ask.other,
                        options = ask.options.map { QuestionOption(it.label, it.note) },
                    )
                }
            },
            createdMs = question.createdMs,
            expiresMs = question.expiresMs,
        )
    }

    fun outcome(outcome: AgentQuestionOutcome): QuestionOutcome = when (outcome) {
        AgentQuestionOutcome.ANSWERED -> QuestionOutcome.ANSWERED
        AgentQuestionOutcome.DISMISSED -> QuestionOutcome.DISMISSED
        AgentQuestionOutcome.EXPIRED -> QuestionOutcome.EXPIRED
        AgentQuestionOutcome.CANCELLED -> QuestionOutcome.CANCELLED
    }

    fun answers(answers: List<QuestionAnswer>): List<AgentAnswer> =
        answers.map { AgentAnswer(id = it.id, labels = it.labels, other = it.other) }

    /**
     * The questions waiting, as the timeline takes them (`AgentTimeline.rows`'
     * `questions`: how many each asks). A secret one goes without its asks.
     */
    fun timelineQuestions(questions: List<QuestionInfo>): List<AgentQuestion> = questions.map { question ->
        AgentQuestion(
            token = question.token,
            thread = question.threadId,
            threadTitle = "",
            agent = question.agent,
            item = question.itemId,
            secret = question.secret,
            asks = question.asks.map { ask ->
                AgentQuestionAsk(
                    id = ask.id,
                    header = ask.header,
                    text = ask.text,
                    multi = ask.multi,
                    other = ask.other,
                    secret = false,
                    options = ask.options.map { AgentQuestionOption(label = it.label, note = it.note) },
                )
            },
            createdMs = question.createdMs,
            expiresMs = question.expiresMs,
        )
    }

    /** The images a user item carries, by id; the PC holds them, the phone reads their thumbnails. */
    fun imageIds(item: AgentItem): List<String> = item.images.map { it.id }

    fun dispatch(followUp: FollowUp): AgentDispatch = when (followUp) {
        FollowUp.STEER -> AgentDispatch.STEER
        FollowUp.QUEUE -> AgentDispatch.QUEUE
    }

    private fun ULong.toLongClamped(): Long = if (this > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else toLong()
}
