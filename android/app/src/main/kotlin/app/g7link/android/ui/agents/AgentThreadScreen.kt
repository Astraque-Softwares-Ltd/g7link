// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.text.format.DateFormat
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.notifications.AgentLimitAlarm
import app.g7link.android.ui.agents.voice.AgentMicButton
import app.g7link.android.ui.agents.voice.AgentVoice
import app.g7link.android.ui.agents.voice.AgentVoiceStrip
import app.g7link.android.ui.agents.voice.rememberAgentDictation
import app.g7link.android.ui.agents.voice.rememberTapToTalk
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.g7Collapse
import app.g7link.android.ui.theme.g7Expand
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.mono
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentCheckpointWhy
import app.g7link.core.AgentLink
import app.g7link.core.AgentPlanStatus
import app.g7link.core.AgentPlanStep
import app.g7link.core.AgentRow
import app.g7link.core.AgentRowKind
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import java.util.Date
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How far opening the thread got. */
private enum class ThreadLoad { LOADING, READY, NOT_ALLOWED, UNAVAILABLE, UNREACHABLE, GONE, FAILED }

/** The rows the user opened, kept across a rotation. */
private val OPEN_KEYS = listSaver<Set<String>, String>(save = { it.toList() }, restore = { it.toSet() })

/** Diff rows the request sheet shows; a longer change is reviewed full-screen. */
private const val SHEET_DIFF_ROWS = 60

/** Pages read back at most to reach a search hit's message. */
private const val JUMP_PAGES = 10

/** How long after opening the reply box is focused for a limit reset's Open. */
private const val FOCUS_AFTER_MS = 300L

/**
 * One thread on a PC (contract §P6, the phone board): its rows from the FFI,
 * the request it waits on as a sheet over the reply box with Stop beside send,
 * "Deny and send" when the box has words, the plan pinned above, and full-screen
 * edits, output and changes. Phase 3 (§U9.3): a long-press on a message for
 * Edit from here, Rewind to here (the conversation only) and Fork from here;
 * Undo on a rewind's notice; the draft kept on the PC; and, while the PC
 * can't be reached, what this phone saved with the messages held for it.
 */
@Composable
fun AgentThreadScreen(
    device: Device,
    core: Core,
    threadId: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val resources = LocalResources.current
    val context = LocalContext.current
    val all by core.agents.collectAsStateWithLifecycle()
    val agents = all[device.id] ?: AgentsState()
    val presences by core.agentPresence.collectAsStateWithLifecycle()
    val presence = presences[device.id]
    val openOnPc = remember(presence, threadId) { presence?.shows(threadId, System.currentTimeMillis()) == true }
    val thread = agents.thread(threadId)
    val open = agents.open?.takeIf { it.threadId == threadId }
    val hasCatalog = agents.catalog != null
    val canSayWhy = agents.catalog?.canSayWhy == true
    var load by remember(threadId) { mutableStateOf(ThreadLoad.LOADING) }
    var attempt by remember(threadId) { mutableIntStateOf(0) }
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val now = rememberNow()
    val offline = !device.online

    // Leaving the screen drops the transcript.
    DisposableEffect(device.id, threadId) {
        onDispose { core.agentClosed(device.id, threadId) }
    }
    // While it's in front the PC sends its text and its notifications wait; read it again on coming back.
    LaunchedEffect(device.id, threadId, device.online, hasCatalog, attempt, lifecycle) {
        if (!device.online) {
            load = ThreadLoad.UNREACHABLE
            // What this phone saved is all there is until the PC is back (contract §U9.3 "Offline").
            core.agentSavedThread(device.id, threadId)
            return@LaunchedEffect
        }
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            try {
                load = when (val result = core.agentShown(device.id, threadId)) {
                    is AgentCall.Ok -> ThreadLoad.READY
                    is AgentCall.Failed -> when (result.failure) {
                        AgentFailure.DENIED -> ThreadLoad.NOT_ALLOWED
                        AgentFailure.UNSUPPORTED -> ThreadLoad.UNAVAILABLE
                        AgentFailure.NOT_FOUND -> ThreadLoad.GONE
                        AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> ThreadLoad.UNREACHABLE
                        AgentFailure.OTHER -> ThreadLoad.FAILED
                    }
                }
                awaitCancellation()
            } finally {
                core.agentHidden(device.id, threadId)
            }
        }
    }

    // ---- The typing guard (contract §P8.2): a request that comes while you type waits for a pause ----
    val guard = remember(threadId) { FfiTypingGuard() }
    DisposableEffect(guard) { onDispose { guard.close() } }
    var guardTick by remember(threadId) { mutableIntStateOf(0) }
    // Requests and questions alike: one that comes while you type waits for a pause.
    val pending = remember(agents.asks, agents.questions, threadId) {
        agents.asksFor(threadId).map { it.token } + agents.questionsFor(threadId).map { it.token }
    }
    // Told before anything is drawn for them, so a new request is never shown unheld for a frame.
    val requested = remember(guard, pending) {
        guard.requests(pending, System.currentTimeMillis())
        pending
    }
    val heldTokens = remember(guard, guardTick, requested) { requested.filterTo(HashSet()) { guard.held(it) } }
    var idleAt by remember(threadId) { mutableStateOf<Long?>(null) }
    LaunchedEffect(guard, guardTick, requested) { idleAt = guard.idleAtMs() }
    LaunchedEffect(guard, idleAt) {
        val at = idleAt ?: return@LaunchedEffect
        delay((at - System.currentTimeMillis()).coerceAtLeast(0L))
        guard.idle()
        guardTick++
    }
    // One at a time in the dock: a request or a question, the one asked first (contract §Q6.1).
    val dock = remember(agents.asks, agents.questions, threadId, heldTokens) { agents.dockShown(threadId) { it in heldTokens } }
    val queue = (dock as? DockShown.Approval)?.queue
    val questionQueue = (dock as? DockShown.Question)?.queue

    // ---- The rows (contract §P6.1): made off the main thread whenever their input changes ----
    var openKeys by rememberSaveable(threadId, stateSaver = OPEN_KEYS) { mutableStateOf(emptySet<String>()) }
    val ffiRows = remember(threadId) { FfiRows() }
    DisposableEffect(ffiRows) { onDispose { ffiRows.close() } }
    val feed = remember(ffiRows) { RowsFeed(threadId, ffiRows::make) }
    LaunchedEffect(feed) { feed.start(this) }
    // A turn whose restore point is still pending reads "Recording changes…" a second after it ended: made again then.
    var tick by remember(threadId) { mutableLongStateOf(0L) }
    val recheckAt = remember(open?.turns, tick) { AgentTurns.recheckAtMs(open?.turns.orEmpty(), System.currentTimeMillis()) }
    LaunchedEffect(recheckAt) {
        val at = recheckAt ?: return@LaunchedEffect
        delay((at - System.currentTimeMillis()).coerceAtLeast(0L))
        tick++
    }
    val input = remember(agents, threadId, openKeys, heldTokens, tick) {
        AgentsModel.timelineInput(agents, threadId, openKeys) { it in heldTokens }?.copy(tick = tick)
    }
    LaunchedEffect(feed, input) { input?.let(feed::submit) }
    val made by feed.rows.collectAsStateWithLifecycle()
    val rows = made?.takeIf { it.input.key == threadId }?.rows.orEmpty()
    val itemsById = remember(open?.items) { open?.items.orEmpty().associateBy { it.id } }
    val folder = thread?.folder.orEmpty()

    val agentName = thread?.let { agents.agentName(it.agent) }.orEmpty()
    val status = thread?.let { AgentsModel.status(it) }
    val running = thread != null && AgentsModel.running(thread)
    val codex = thread?.agent == "codex"
    val abilities = remember(agents.catalog, thread?.agent) { thread?.let { agents.abilities(it.agent) } ?: AgentAbilities() }
    val control = remember(thread) { thread?.let(AgentControlFfi::control) ?: ThreadControl() }
    val settingsNow = remember(control) { AgentSettings.of(control) }
    var stopping by remember(threadId) { mutableStateOf(false) }
    LaunchedEffect(running) { if (!running) stopping = false }
    // A share from another app (contract §V6): its text goes in the box, its pictures upload as attachments; nothing is sent.
    val shared = remember(device.id, threadId) { AgentShared.take(device.id, threadId) }
    // A limit reset's Open: the reply box focused, nothing typed.
    val focusReply = remember(device.id, threadId) { AgentFocus.take(device.id, threadId) }
    var draft by rememberSaveable(threadId) { mutableStateOf(shared?.text.orEmpty()) }
    var draftFor by rememberSaveable(threadId) { mutableStateOf<String?>(null) }
    var sending by remember(threadId) { mutableStateOf(false) }
    var loadingEarlier by remember(threadId) { mutableStateOf(false) }
    var sharing by remember(threadId) { mutableStateOf(false) }
    var full by remember(threadId) { mutableStateOf<FullView?>(null) }
    var link by remember(threadId) { mutableStateOf<PendingLink?>(null) }
    var sheetCollapsed by remember(threadId) { mutableStateOf(false) }
    LaunchedEffect(queue?.shown?.token, questionQueue?.shown?.token) { sheetCollapsed = false }
    val retry: () -> Unit = { attempt++ }

    // ---- A question in the dock: the answers chosen, the reply box its "Something else" field ----
    val question = questionQueue?.shown
    var answersState by remember(threadId) { mutableStateOf<QuestionAnswers?>(null) }
    val answers = question?.let { QuestionDock.answersFor(it, answersState) }
    // Words typed for a question don't outlive it: another question, or none, starts empty.
    LaunchedEffect(question?.token) { if (answersState?.token != question?.token) answersState = null }
    val questionField = if (question != null && answers != null) QuestionDock.field(question, answers) else null
    val replyingShown = question != null && question.token in agents.replying

    // ---- Images for the next message, and the sheets ----
    var images by remember(threadId) { mutableStateOf(emptyList<PendingImage>()) }
    val thumbs = remember(device.id) { ImageThumbs { id -> (core.agentImageThumb(device.id, id) as? AgentCall.Ok)?.value } }
    var imageSheet by remember(threadId) { mutableStateOf(false) }
    var settingsOpen by remember(threadId) { mutableStateOf(false) }
    var modeOpen by remember(threadId) { mutableStateOf(false) }
    // The plan whose Implement was tapped: the modes it may run in are offered.
    var implementFor by remember(threadId) { mutableStateOf<String?>(null) }
    var details by remember(threadId) { mutableStateOf(false) }
    var editing by remember(threadId) { mutableStateOf<QueuedLine?>(null) }
    var queueBusy by remember(threadId) { mutableStateOf(emptySet<String>()) }
    var implementing by remember(threadId) { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    val composerSelection = remember(threadId) { TextSelection() }
    val imagesAllowed = AgentSettings.takesImages(abilities, settingsNow)
    val newestPlan = remember(open?.items) { AgentsModel.newestProposedPlan(open?.items.orEmpty()) }
    val limit = AgentUsage.pinned(abilities.limits)

    val paused = status as? ThreadStatus.Paused
    val pausedUntil = paused?.resetsMs?.takeIf { it > now }
    val answeringShown = queue != null && queue.shown.token in agents.answering
    val composer = AgentDock.composer(
        ComposerInput(
            running = running,
            hasThread = thread != null,
            queue = queue,
            canSayWhy = canSayWhy,
            draft = draft,
            draftFor = draftFor,
            answering = answeringShown,
            sending = sending,
            paused = paused != null,
            pausedUntilMs = pausedUntil,
            codex = codex,
            waiting = thread?.state == AgentThreadState.WAITING || dock != null,
            followUp = AgentFollowUp.primary(abilities),
            queueRoom = AgentQueue.hasRoom(control),
            question = questionField,
            images = AgentImages.readyIds(images).size,
            uploading = AgentImages.uploading(images),
            imagesAllowed = imagesAllowed,
            offline = offline,
        ),
    )

    // ---- Phase 3: the message actions, rewind, undo, the draft and what waits on this phone ----
    var actionsFor by remember(threadId) { mutableStateOf<AgentRow?>(null) }
    var rewind by remember(threadId) { mutableStateOf<RewindSheet?>(null) }
    var rewindAsk by remember(threadId) { mutableIntStateOf(0) }
    var undoing by remember(threadId) { mutableStateOf(false) }
    var editingMessage by remember(threadId) { mutableStateOf(false) }
    var forking by remember(threadId) { mutableStateOf(false) }
    var heldBusy by remember(threadId) { mutableStateOf(emptySet<String>()) }
    var drafts by remember(threadId) { mutableStateOf(DraftState()) }
    var composerFocused by remember(threadId) { mutableStateOf(false) }
    // A search hit opened the thread at a message: scrolled to, and lit for a moment.
    val searchHit = remember(device.id, threadId) { AgentJump.take(device.id, threadId) }
    // Handed over from the PC (contract §V8.5): opened where the PC showed it, with the draft from there.
    val handedOver = remember(device.id, threadId) { AgentHandoffs.take(device.id, threadId) }
    val jumpTo = searchHit ?: handedOver?.seq
    var lit by remember(threadId) { mutableStateOf(searchHit) }
    // What the PC's draft put in the box: "Draft from <PC>" over it while it's still that.
    var handoffDraft by remember(threadId) { mutableStateOf<String?>(null) }
    // "Continue on <PC>" while the PC is locked: it opens there once it's unlocked.
    var handoffLocked by remember(threadId) { mutableStateOf(false) }
    var handingOver by remember(threadId) { mutableStateOf(false) }
    val heldHere = remember(agents.held, threadId) { AgentOutbox.of(agents.held, threadId) }
    val readonly = thread?.readonly != null
    val drafting = agents.catalog?.canControl == true
    val questionWaiting = questionQueue != null

    // ---- The phone beside the PC (contract §V9.1): a mode of this screen; the PC may switch its pane, never open it ----
    var beside by rememberSaveable(threadId) { mutableStateOf<BesidePane?>(null) }
    val offerBeside = remember(presence, threadId) { AgentBeside.offered(presence, threadId, System.currentTimeMillis()) }
    val shownOnPc = remember(presence, threadId) { AgentBeside.shownOnPc(presence, threadId, System.currentTimeMillis()) }
    // Shown only while the thread is: otherwise the thread screen says why, and the PC hears "the thread".
    val besideShown = beside?.takeIf { thread != null && load == ThreadLoad.READY && !offline }
    LaunchedEffect(besideShown) { core.agentBeside(device.id, threadId, AgentBeside.wire(besideShown)) }
    // It goes back to the thread once the PC hasn't shown it for more than a minute.
    var besideStoppedAt by remember(threadId) { mutableStateOf<Long?>(null) }
    LaunchedEffect(besideShown != null, shownOnPc) {
        if (besideShown == null) {
            besideStoppedAt = null
            return@LaunchedEffect
        }
        val stopped = AgentBeside.stoppedAt(besideStoppedAt, shownOnPc, System.currentTimeMillis())
        besideStoppedAt = stopped
        val at = AgentBeside.leaveAtMs(stopped) ?: return@LaunchedEffect
        delay((at - System.currentTimeMillis()).coerceAtLeast(0L) + 1L)
        beside = null
    }
    // The PC's menu switches the pane of a phone beside it; "the thread" takes it back.
    LaunchedEffect(device.id, threadId, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            core.agentBesideAsks.collect { ask ->
                if (ask.pcId == device.id) beside = AgentBeside.follow(beside, threadId, ask.threadId, ask.pane)
            }
        }
    }
    // "Change the plan" or "Something else" from beside: the reply box, once it's back.
    var focusAfterBeside by remember(threadId) { mutableIntStateOf(0) }

    // ---- A dev server the agent started (contract §V9.2): its address on this link, while the link is on the network ----
    val ports = thread?.ports.orEmpty()
    val linkPath = (device.link as? app.g7link.core.Link.Online)?.let { Triple(it.relayed, it.usb, it.tailnet) }
    val portLine by produceState<PortLine?>(null, ports, linkPath) {
        val shown = AgentPorts.shown(ports)
        val url = if (shown != null && shown.lan && linkPath != null) core.agentPortUrl(device.id, shown.port) else null
        value = AgentPorts.line(ports, online = linkPath != null) { port -> url.takeIf { port == shown } }
    }

    val copy = rememberCopy { core.say(resources.getString(R.string.agents_copied)) }
    val guardChanged: () -> Unit = { guardTick++ }
    val stop: () -> Unit = {
        stopping = true
        scope.launch {
            if (core.agentCancel(device.id, threadId) is AgentCall.Failed) {
                stopping = false
                core.say(resources.getString(R.string.agents_failed_generic))
            }
        }
    }
    val answer: (Boolean) -> Unit = answer@{ allow ->
        val ask = queue?.shown ?: return@answer
        scope.launch {
            val result = core.agentAnswer(device.id, ask.token, allow)
            if (result is AgentCall.Failed) {
                // Gone: someone answered first, or it lapsed.
                val message = if (result.failure == AgentFailure.NOT_FOUND) R.string.agents_answered_elsewhere else R.string.agents_failed_generic
                core.say(resources.getString(message))
            }
        }
    }
    val failedSay: (AgentFailure) -> String = { failure ->
        when (failure) {
            AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_thread_gone)
            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
            // The PC's BUSY: it started working again (from the PC) before this reached it.
            AgentFailure.OTHER -> if (running) {
                resources.getString(R.string.agents_reply_when_stops, agentName)
            } else {
                resources.getString(R.string.agents_failed_generic)
            }
        }
    }
    // Each send takes the transcript back to its end (AgentRowsList's jump).
    var jumps by remember(threadId) { mutableIntStateOf(0) }
    val send: (FollowUp?) -> Unit = send@{ chosen ->
        if (!composer.actionEnabled || sending) return@send
        val text = draft.trim()
        val shown = queue?.shown
        sending = true
        // Back to the end, where what was sent and what answers it come in.
        jumps++
        scope.launch {
            if (composer.action == ComposerAction.DENY_AND_SEND && shown != null) {
                // The words go with a denial of the request they were typed under (contract §P8.3).
                when (val result = core.agentAnswer(device.id, shown.token, false, text)) {
                    is AgentCall.Ok -> {
                        draft = ""
                        draftFor = null
                        guard.sent()
                        guardChanged()
                    }
                    is AgentCall.Failed -> core.say(
                        if (result.failure == AgentFailure.NOT_FOUND) {
                            resources.getString(R.string.agents_answered_elsewhere)
                        } else {
                            resources.getString(R.string.agents_failed_generic)
                        },
                    )
                }
            } else {
                // While it works: steer it or queue the message (long-press chose the other); else a turn.
                // Words wait on this phone when the PC can't be reached; pictures can't (contract §U8.3).
                val followUp = if (composer.action == ComposerAction.FOLLOW_UP) chosen ?: composer.followUp else null
                val ids = AgentImages.readyIds(images)
                when (val result = core.agentSend(device.id, threadId, text, followUp, ids, hold = ids.isEmpty())) {
                    is AgentCall.Ok -> {
                        // Held on this phone while the PC can't be reached, it shows under the rows: there too.
                        jumps++
                        draft = ""
                        draftFor = null
                        images = emptyList()
                        drafts = AgentDrafts.sent(drafts)
                        editingMessage = false
                        guard.sent()
                        guardChanged()
                    }
                    is AgentCall.Failed -> core.say(
                        if (ids.isNotEmpty() && (result.failure == AgentFailure.OFFLINE || result.failure == AgentFailure.TIMEOUT)) {
                            resources.getString(R.string.agents_pictures_wait, device.name)
                        } else {
                            failedSay(result.failure)
                        },
                    )
                }
            }
            sending = false
        }
    }
    val typed: (String) -> Unit = { text ->
        if (question != null && answers != null) {
            // The reply box is the question's "Something else": its words are never a draft.
            if (questionField == QuestionField.OTHER) answersState = QuestionDock.typeOther(question, answers, text)
        } else {
            draft = text
            draftFor = AgentDock.editedUnder(queue)
            drafts = AgentDrafts.edited(drafts)
        }
        guard.typed(System.currentTimeMillis())
        guardChanged()
    }

    // ---- Source control (phase 4 §SC9.3): the strip, its sheet, review and commit, push, the pull request, branches ----
    val strip = agents.gitStrip(threadId)
    var gitStatus by remember(threadId) { mutableStateOf<GitStatus?>(null) }
    var gitSavedMs by remember(threadId) { mutableStateOf<Long?>(null) }
    var prView by remember(threadId) { mutableStateOf<PrView?>(null) }
    var gitSheet by remember(threadId) { mutableStateOf(false) }
    var gitRead by remember(threadId) { mutableIntStateOf(0) }
    var gitRefresh by remember(threadId) { mutableStateOf<String?>(null) }
    var prRefresh by remember(threadId) { mutableStateOf(false) }
    // ---- Project scripts (phase 6 §X5.2): the folder's scripts and the thread's runs ----
    var scriptsSheet by remember(threadId) { mutableStateOf(false) }
    val scriptRuns = remember(agents.scriptRuns, thread) { agents.runsOf(threadId) }
    val scriptsFolder = remember(agents.catalog, thread?.folder) { agents.scriptsFolderOf(threadId) }
    val runningScript = AgentScripts.runningLine(scriptRuns, scriptsFolder, offline)
    var branchSheet by remember(threadId) { mutableStateOf(false) }
    var pushAsk by remember(threadId) { mutableStateOf<PushRow.Ready?>(null) }
    val pushAllowed = AgentGitFfi.pushAllowed(agents.catalog)
    val threadBusy = thread != null && (running || thread.state == AgentThreadState.WAITING)
    // The folder as git sees it: read when the thread opens and when the sheet does; offline, the strip this phone kept.
    LaunchedEffect(device.id, threadId, device.online, gitRead) {
        if (device.online) {
            gitStatus = (core.agentGitStatus(device.id, threadId) as? AgentCall.Ok)?.value ?: gitStatus
            gitSavedMs = null
        } else {
            gitSavedMs = core.agentGitCached(device.id, threadId)
        }
    }
    // The pull request is read (GitHub may be asked) only when the sheet opens or Refresh is pressed.
    LaunchedEffect(device.id, threadId, gitSheet, gitRead, device.online) {
        if (!gitSheet || !device.online) return@LaunchedEffect
        val refresh = prRefresh
        prRefresh = false
        prView = (core.agentPrView(device.id, threadId, refresh) as? AgentCall.Ok)?.value ?: prView
    }
    // Refresh: a fetch from the remote, then the folder and the pull request read again.
    val refreshJob = gitRefresh?.let { agents.gitJobs[it] }
    LaunchedEffect(refreshJob?.phase) {
        if (gitRefresh != null && refreshJob != null && refreshJob.phase != JobPhase.RUNNING) {
            gitRefresh = null
            gitRead++
        }
    }
    val gitRefreshNow: () -> Unit = {
        if (gitRefresh == null) {
            prRefresh = true
            scope.launch {
                when (val started = core.agentGitFetch(device.id, threadId)) {
                    is GitStart.Started -> gitRefresh = started.job.id
                    // No remote to fetch from: the folder is read again all the same.
                    is GitStart.Refused -> gitRead++
                }
            }
        }
    }
    val startPush: (PushRow.Ready, String?) -> Unit = { push, remote ->
        pushAsk = null
        scope.launch {
            when (val started = core.agentGitPush(device.id, threadId, remote)) {
                is GitStart.Started -> full = FullView.Push(started.job.id, push.branch, remote ?: push.remote.orEmpty())
                is GitStart.Refused -> {
                    val error = started.error
                    if (error?.reason == "pick_remote" && error.names.isNotEmpty()) {
                        pushAsk = push.copy(remote = null, remotes = error.names)
                    } else {
                        core.say(
                            when {
                                error != null -> resources.getString(R.string.agents_git_push_failed)
                                started.failure == AgentFailure.DENIED -> resources.getString(R.string.agents_git_switch_off, device.name)
                                else -> failedSay(started.failure)
                            },
                        )
                    }
                }
            }
        }
    }
    // A comment on diff lines: a chip in the reply box, at most five a message.
    val addQuote: (Quote) -> Unit = { quote ->
        val next = AgentQuotes.add(draft, quote)
        if (next == null) {
            core.say(resources.getString(R.string.agents_git_quotes_max))
        } else {
            draft = next
            draftFor = AgentDock.editedUnder(queue)
            drafts = AgentDrafts.edited(drafts)
            full = null
            runCatching { focus.requestFocus() }
        }
    }

    // ---- Phase 2 actions (contract §Q3.2 through the phone's wire, §Q5) ----
    val addImages: (List<android.net.Uri>, String?) -> Unit = { uris, cameraPath ->
        val room = AgentImages.room(images)
        if (room <= 0) AgentImageIntake.deleteCameraFile(cameraPath)
        uris.take(room).forEachIndexed { index, uri ->
            val key = java.util.UUID.randomUUID().toString()
            images = AgentImages.adding(images, key)
            val path = if (index == 0) cameraPath else null
            scope.launch {
                val problem = when (val read = AgentImageIntake.read(context, uri)) {
                    is ImageRead.Problem -> read.problem
                    is ImageRead.Bytes -> when (val sent = core.agentImageSend(device.id, read.name, read.bytes)) {
                        is AgentCall.Ok -> {
                            images = AgentImages.uploaded(images, key, sent.value)
                            null
                        }
                        is AgentCall.Failed -> ImageProblem.FAILED
                    }
                }
                // A photo taken for it lives only until it's sent to the PC.
                AgentImageIntake.deleteCameraFile(path)
                if (problem != null) {
                    images = AgentImages.removed(images, key)
                    core.say(
                        resources.getString(
                            when (problem) {
                                ImageProblem.NOT_AN_IMAGE -> R.string.agents_image_not_supported
                                ImageProblem.TOO_LARGE -> R.string.agents_image_too_large
                                ImageProblem.FAILED -> R.string.agents_image_failed
                            },
                        ),
                    )
                }
            }
        }
    }
    val pickers = rememberImagePickers(onPicked = addImages, onCameraFailed = { core.say(resources.getString(R.string.agents_image_failed)) })
    var sharedImages by remember(threadId) { mutableStateOf(shared?.images.orEmpty()) }
    LaunchedEffect(shared) {
        if (shared == null) return@LaunchedEffect
        if (shared.text.isNotEmpty()) drafts = AgentDrafts.edited(drafts)
        if (shared.shortened) core.say(resources.getString(R.string.agents_share_shortened))
    }
    // The shared pictures wait for the PC to say what the thread's model takes.
    LaunchedEffect(sharedImages, thread != null, hasCatalog) {
        val paths = sharedImages
        if (paths.isEmpty() || thread == null || !hasCatalog) return@LaunchedEffect
        sharedImages = emptyList()
        if (imagesAllowed) {
            // Each copy goes once it's uploaded (or refused).
            paths.forEach { path -> addImages(listOf(android.net.Uri.fromFile(java.io.File(path))), path) }
        } else {
            paths.forEach(AgentImageIntake::deleteCameraFile)
            core.say(resources.getString(R.string.agents_image_model, agentName))
        }
    }
    val sharedLeft by rememberUpdatedState(sharedImages)
    DisposableEffect(threadId) { onDispose { sharedLeft.forEach(AgentImageIntake::deleteCameraFile) } }
    LaunchedEffect(focusReply) {
        if (!focusReply) return@LaunchedEffect
        // Once the reply box is laid out.
        delay(FOCUS_AFTER_MS)
        runCatching { focus.requestFocus() }
    }
    LaunchedEffect(focusAfterBeside) {
        if (focusAfterBeside == 0) return@LaunchedEffect
        delay(FOCUS_AFTER_MS)
        runCatching { focus.requestFocus() }
    }
    val openSettings: () -> Unit = {
        when (composer.settings) {
            SettingsAccess.OPEN -> settingsOpen = true
            SettingsAccess.LATER -> core.say(resources.getString(R.string.agents_settings_later, agentName))
            SettingsAccess.INERT -> Unit
        }
    }
    val openMode: () -> Unit = {
        when (composer.settings) {
            SettingsAccess.OPEN -> modeOpen = true
            SettingsAccess.LATER -> core.say(resources.getString(R.string.agents_settings_later, agentName))
            SettingsAccess.INERT -> Unit
        }
    }
    val configure: (ThreadSettings) -> Unit = { chosen ->
        settingsOpen = false
        modeOpen = false
        val change = AgentSettings.change(settingsNow, chosen)
        if (change != null) {
            scope.launch {
                when (val result = core.agentConfigure(device.id, threadId, change)) {
                    is AgentCall.Ok -> Unit
                    is AgentCall.Failed -> core.say(
                        if (result.failure == AgentFailure.OTHER) resources.getString(R.string.agents_settings_later, agentName) else failedSay(result.failure),
                    )
                }
            }
        }
    }
    val queueOp: (String, suspend () -> AgentCall<Unit>) -> Unit = { id, op ->
        queueBusy = queueBusy + id
        scope.launch {
            val result = op()
            queueBusy = queueBusy - id
            if (result is AgentCall.Failed) {
                core.say(if (result.failure == AgentFailure.NOT_FOUND) resources.getString(R.string.agents_queue_already_sent) else failedSay(result.failure))
            }
        }
    }
    val sendQuestion: () -> Unit = sendQuestion@{
        val shown = question ?: return@sendQuestion
        val chosen = answers ?: return@sendQuestion
        val out = QuestionDock.answersOut(shown, chosen) ?: return@sendQuestion
        scope.launch {
            when (val result = core.agentReply(device.id, shown.token, out)) {
                is AgentCall.Ok -> {
                    answersState = null
                    guard.sent()
                    guardChanged()
                }
                is AgentCall.Failed -> core.say(
                    resources.getString(if (result.failure == AgentFailure.NOT_FOUND) R.string.agents_answered_elsewhere else R.string.agents_failed_generic),
                )
            }
        }
    }
    val dismissQuestion: () -> Unit = dismissQuestion@{
        val shown = question ?: return@dismissQuestion
        scope.launch {
            when (val result = core.agentDismiss(device.id, shown.token)) {
                is AgentCall.Ok -> answersState = null
                is AgentCall.Failed -> core.say(
                    resources.getString(if (result.failure == AgentFailure.NOT_FOUND) R.string.agents_answered_elsewhere else R.string.agents_failed_generic),
                )
            }
        }
    }
    // One tap: the mode the folder remembers (the PC picks it); the arrow offers the others.
    val implementIn: (String, ThreadMode?) -> Unit = { itemId, mode ->
        implementFor = null
        if (!implementing) {
            implementing = true
            scope.launch {
                val result = core.agentImplement(device.id, threadId, itemId, mode)
                implementing = false
                if (result is AgentCall.Failed) core.say(failedSay(result.failure))
            }
        }
    }
    val implement: (String) -> Unit = { itemId -> implementIn(itemId, null) }
    val implementMenu: (String) -> Unit = { itemId -> if (!implementing) implementFor = itemId }
    val implementLatest by rememberUpdatedState(implement)
    val implementMenuLatest by rememberUpdatedState(implementMenu)
    val changePlan: () -> Unit = { runCatching { focus.requestFocus() } }
    val focused: (Boolean) -> Unit = { on ->
        if (on) guard.focus() else guard.blur()
        composerFocused = on
        guardChanged()
    }
    val loadEarlier: () -> Unit = {
        val first = open?.items?.firstOrNull()?.seq
        if (first != null && !loadingEarlier) {
            loadingEarlier = true
            scope.launch {
                core.agentRead(device.id, threadId, first)
                loadingEarlier = false
            }
        }
    }
    val share: () -> Unit = {
        val title = thread?.title.orEmpty()
        if (!sharing) {
            sharing = true
            scope.launch {
                when (val result = core.agentExport(device.id, threadId, null)) {
                    is AgentCall.Ok -> if (!AgentShare.share(context, result.value, title)) {
                        core.say(resources.getString(R.string.agents_share_failed))
                    }
                    is AgentCall.Failed -> core.say(
                        when (result.failure) {
                            AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
                            AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
                            AgentFailure.NOT_FOUND -> resources.getString(R.string.agents_thread_gone)
                            AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                            AgentFailure.OTHER -> resources.getString(R.string.agents_share_failed)
                        },
                    )
                }
                sharing = false
            }
        }
    }
    // ---- The draft on the PC (phase 2 §Q6.1): read on open, written after a pause, never while a question waits ----
    val draftKey = AgentDrafts.key(threadId)
    // ---- "Continue on <PC>" (contract §V8.5): the draft goes first, then the PC opens the thread where this screen is ----
    val listState = rememberLazyListState()
    val handOver: () -> Unit = handOver@{
        if (handingOver || offline) return@handOver
        handingOver = true
        val seq = firstSeqShown(listState, rows, earlier = open?.let { it.moreBefore && it.items.isNotEmpty() } == true)
        scope.launch {
            if (drafting && AgentDrafts.mayWrite(drafts, questionWaiting)) {
                val text = draft.take(AgentDrafts.MAX_BYTES)
                val saved = core.agentDraftSet(device.id, draftKey, text, AgentImages.readyIds(images), drafts.rev)
                if (saved is AgentCall.Ok) drafts = AgentDrafts.saved(drafts, saved.value, text)
            }
            when (val result = core.agentHandoffTo(device.id, threadId, seq)) {
                is AgentCall.Ok -> handoffLocked = result.value == AgentHandoffs.Sent.LOCKED
                is AgentCall.Failed -> core.say(failedSay(result.failure))
            }
            handingOver = false
        }
    }
    // The PC opens it at its unlock (or the hand-off lapses): the line goes then.
    LaunchedEffect(handoffLocked, presence) {
        if (!handoffLocked) return@LaunchedEffect
        if (presence != null && presence.fresh(System.currentTimeMillis()) && !presence.locked) {
            handoffLocked = false
            return@LaunchedEffect
        }
        delay(AgentHandoffs.LIFE_MS)
        handoffLocked = false
    }
    LaunchedEffect(device.id, threadId, device.online, drafting) {
        if (!device.online || !drafting) return@LaunchedEffect
        val read = core.agentDraftGet(device.id, draftKey) as? AgentCall.Ok ?: return@LaunchedEffect
        val step = AgentDrafts.loaded(drafts, read.value, draft)
        drafts = step.state
        step.text?.let { draft = it }
        step.images?.let { ids -> images = ids.map { PendingImage.Ready(it, it) } }
        if (handedOver != null) handoffDraft = step.text
    }
    LaunchedEffect(draft, drafts.dirty, questionWaiting, device.online) {
        if (!device.online || !drafting || !AgentDrafts.mayWrite(drafts, questionWaiting)) return@LaunchedEffect
        delay(AgentDrafts.SAVE_AFTER_MS)
        val text = draft.take(AgentDrafts.MAX_BYTES)
        val saved = core.agentDraftSet(device.id, draftKey, text, AgentImages.readyIds(images), drafts.rev)
        if (saved is AgentCall.Ok) {
            // Typed again while it was on its way: still to be written.
            val next = AgentDrafts.saved(drafts, saved.value, text)
            drafts = if (draft.take(AgentDrafts.MAX_BYTES) != text) AgentDrafts.edited(next) else next
        }
    }
    // Changed on another device: taken while the box isn't focused, offered while it is.
    val changedRev = agents.draftChanges[draftKey]
    LaunchedEffect(changedRev, device.online) {
        val rev = changedRev ?: return@LaunchedEffect
        if (!device.online || !drafting || questionWaiting || !AgentDrafts.stale(drafts, rev)) return@LaunchedEffect
        val read = core.agentDraftGet(device.id, draftKey) as? AgentCall.Ok ?: return@LaunchedEffect
        val step = AgentDrafts.arrived(drafts, read.value, composerFocused, draft)
        drafts = step.state
        step.text?.let { draft = it }
        step.images?.let { ids -> images = ids.map { PendingImage.Ready(it, it) } }
    }
    val leaving by rememberUpdatedState(Triple(draft, drafts, questionWaiting))
    val leavingImages by rememberUpdatedState(AgentImages.readyIds(images))
    val leavingDrafting by rememberUpdatedState(drafting && device.online)
    DisposableEffect(device.id, threadId) {
        onDispose {
            val (text, state, asking) = leaving
            if (leavingDrafting && AgentDrafts.mayWrite(state, asking)) {
                core.agentDraftLeave(device.id, draftKey, text.take(AgentDrafts.MAX_BYTES), leavingImages, state.rev)
            }
        }
    }

    // ---- A search hit: the transcript is read back to that message, then scrolled to and lit ----
    LaunchedEffect(jumpTo, load) {
        val target = jumpTo ?: return@LaunchedEffect
        if (load != ThreadLoad.READY) return@LaunchedEffect
        var pages = 0
        while (pages++ < JUMP_PAGES) {
            val shown = core.agents.value[device.id]?.open?.takeIf { it.threadId == threadId } ?: break
            val first = shown.items.firstOrNull()?.seq ?: break
            if (first <= target || !shown.moreBefore) break
            if (core.agentRead(device.id, threadId, first) is AgentCall.Failed) break
        }
    }
    val litShown = lit != null && rows.any { it.seq == lit }
    LaunchedEffect(litShown) {
        if (!litShown) return@LaunchedEffect
        delay(AgentJump.HIGHLIGHT_MS)
        lit = null
    }

    // ---- Edit from here, Rewind to here, Fork from here (board 20, 21) ----
    val openRewind: (UInt, Boolean) -> Unit = { turn, refill ->
        rewind = AgentRewind.opened(turn, refill)
        rewindAsk++
    }
    LaunchedEffect(rewindAsk) {
        val asking = rewind as? RewindSheet.Loading ?: return@LaunchedEffect
        rewind = when (val plan = core.agentRewindPreview(device.id, threadId, asking.turn)) {
            is AgentCall.Ok -> AgentRewind.previewed(rewind, asking.turn, plan.value)
            is AgentCall.Failed -> AgentRewind.previewFailed(rewind, asking.turn, plan.failure)
        }
    }
    val applyRewind: () -> Unit = apply@{
        val ready = rewind as? RewindSheet.Ready ?: return@apply
        if (!AgentRewind.canApply(ready, System.currentTimeMillis())) return@apply
        rewind = AgentRewind.applying(ready)
        scope.launch {
            when (val result = core.agentRewindApply(device.id, ready.plan.token, ready.refill)) {
                is AgentCall.Ok -> {
                    rewind = null
                    if (ready.refill) {
                        // Edit from here: the message comes back through the draft, whatever the box held.
                        val back = core.agentDraftGet(device.id, draftKey) as? AgentCall.Ok
                        val restored = back?.value
                        if (restored != null) {
                            draft = restored.text
                            images = restored.images.map { PendingImage.Ready(it.id, it.id) }
                            drafts = drafts.copy(loaded = true, rev = restored.rev, dirty = false, offer = null)
                        }
                        editingMessage = true
                        runCatching { focus.requestFocus() }
                    }
                }
                is AgentCall.Failed -> {
                    rewind = AgentRewind.applyFailed(rewind, result.failure)
                    if (rewind is RewindSheet.Loading) rewindAsk++
                }
            }
        }
    }
    val undo: () -> Unit = {
        if (!undoing && !offline) {
            undoing = true
            scope.launch {
                when (val result = core.agentRewindUndo(device.id, threadId)) {
                    is AgentCall.Ok -> if (editingMessage) {
                        editingMessage = false
                        draft = ""
                        images = emptyList()
                        drafts = AgentDrafts.edited(drafts)
                    }
                    is AgentCall.Failed -> core.say(failedSay(result.failure))
                }
                undoing = false
            }
        }
    }
    val fork: (UInt) -> Unit = { turn ->
        if (!forking) {
            forking = true
            scope.launch {
                when (val result = core.agentFork(device.id, threadId, turn)) {
                    is AgentCall.Ok -> AgentsNavigation.open(context, device.id, threadId = result.value.id)
                    is AgentCall.Failed -> core.say(failedSay(result.failure))
                }
                forking = false
            }
        }
    }
    val onMessageAction: (AgentRow, MessageAction) -> Unit = { row, action ->
        actionsFor = null
        when (action) {
            MessageAction.COPY -> {
                val text = when (val kind = row.kind) {
                    is AgentRowKind.User -> kind.text
                    else -> itemsById[row.itemId]?.text.orEmpty()
                }
                if (text.isNotEmpty()) copy(text)
            }
            MessageAction.EDIT -> openRewind(row.turn, true)
            MessageAction.REWIND -> openRewind(row.turn, false)
            MessageAction.FORK -> fork(row.turn)
        }
    }
    val heldOp: (String, suspend () -> AgentCall<Unit>) -> Unit = { key, op ->
        heldBusy = heldBusy + key
        scope.launch {
            if (op() is AgentCall.Failed) core.say(resources.getString(R.string.agents_failed_generic))
            heldBusy = heldBusy - key
        }
    }

    val undoLatest by rememberUpdatedState(undo)
    val actions = remember(copy, focus) {
        RowActions(
            toggle = { key -> openKeys = if (key in openKeys) openKeys - key else openKeys + key },
            link = { span, text ->
                // A link whose text is its target opens at once; any other shows where it goes first.
                val shown = span.style.link == AgentLink.SHOWN
                if (shown) openLink(context, span.href)
                if (!shown) link = PendingLink(span.href, text)
            },
            copy = copy,
            output = { itemId -> full = FullView.Output(itemId) },
            diff = { title, doc -> full = FullView.Diff(title, doc) },
            mark = { sheetCollapsed = false },
            implement = { itemId -> implementLatest(itemId) },
            implementIn = { itemId -> implementMenuLatest(itemId) },
            changePlan = changePlan,
            messageActions = { row -> actionsFor = row },
            openChanges = { turn, path -> full = FullView.Changes(turn, path) },
            undo = { undoLatest() },
        )
    }

    val drawer = rememberAgentsDrawer()
    // Beside the PC, Back goes back to the thread.
    BackHandler(enabled = besideShown != null && full == null) { beside = null }
    // A full-screen view closes with Back, and an edit request's when the request is no longer shown.
    BackHandler(enabled = full != null) { full = null }
    val fullEdit = full as? FullView.Edit
    LaunchedEffect(fullEdit, queue?.shown?.token) {
        if (fullEdit != null && queue?.shown?.token != fullEdit.token) full = null
    }
    when (val view = full) {
        is FullView.Edit -> if (queue != null && queue.shown.token == view.token) {
            EditFullScreen(
                queue = queue,
                agentName = agentName,
                folder = folder,
                answering = answeringShown,
                onClose = { full = null },
                onAnswer = { allow ->
                    full = null
                    answer(allow)
                },
            )
            return
        }
        is FullView.Output -> {
            OutputFullScreen(item = itemsById[view.itemId], folder = folder, onClose = { full = null }, onCopy = copy)
            return
        }
        is FullView.Diff -> {
            DiffFullScreen(title = view.title, doc = view.doc, onClose = { full = null })
            return
        }
        is FullView.Changes -> {
            val turn = open?.turns?.firstOrNull { it.turn == view.turn }
            ChangesFullScreen(
                core = core,
                pcId = device.id,
                threadId = threadId,
                title = thread?.title.orEmpty(),
                turn = view.turn,
                newestTurn = maxOf(thread?.turns ?: 0u, open?.turns?.maxOfOrNull { it.turn } ?: 0u),
                turnFiles = turn?.let { (it.files + it.skipped).toInt() },
                startPath = view.path,
                onClose = { full = null },
                pcName = device.name,
                uncommitted = AgentGit.stripShown(strip) && !offline,
                onComment = if (readonly || offline) null else addQuote,
            )
            return
        }
        is FullView.Review -> {
            AgentReviewScreen(
                device = device,
                core = core,
                threadId = threadId,
                initial = view.status,
                thread = thread,
                agentName = agentName,
                pushAllowed = pushAllowed,
                onPush = { push ->
                    full = null
                    pushAsk = push
                },
                onClose = {
                    full = null
                    gitRead++
                },
            )
            return
        }
        is FullView.Pr -> {
            AgentPrScreen(
                device = device,
                core = core,
                threadId = threadId,
                initial = view.status,
                thread = thread,
                agentName = agentName,
                pushAllowed = pushAllowed,
                onClose = {
                    full = null
                    gitRead++
                },
                agents = agents,
                onComment = if (readonly || offline) null else addQuote,
                onAsked = { full = null },
            )
            return
        }
        is FullView.RunOutput -> {
            AgentScriptOutputScreen(device = device, core = core, agents = agents, runId = view.run, onClose = { full = null })
            return
        }
        is FullView.Push -> {
            val job = agents.gitJobs[view.job]
            val pushed = job?.let(AgentJobs::end)
            val openPr: () -> Unit = { gitStatus?.let { status -> full = FullView.Pr(status) } }
            AgentJobScreen(
                title = stringResource(R.string.agents_git_push_title, view.branch, view.remote),
                sub = null,
                job = job,
                running = stringResource(R.string.agents_git_pushing, view.remote),
                pcName = device.name,
                agentName = agentName,
                onStop = { scope.launch { core.agentGitStop(device.id, view.job) } },
                onClose = {
                    full = null
                    gitRead++
                },
                next = if (pushed is JobEnd.Pushed && pushAllowed && gitStatus?.github != null && prView?.pr == null) {
                    stringResource(R.string.agents_git_open_pr) to openPr
                } else {
                    null
                },
            )
            return
        }
        null -> Unit
    }

    // ---- Beside the PC (contract §V9.1): Changes, Plan and Requests; the diffs above open over it ----
    val besideNow = besideShown
    if (besideNow != null && thread != null) {
        val waiting = remember(agents.asks, agents.questions, threadId) { AgentBeside.requests(agents.asksFor(threadId), agents.questionsFor(threadId)) }
        val besideActions = remember(actions) {
            RowActions(
                toggle = actions.toggle,
                link = actions.link,
                copy = actions.copy,
                output = actions.output,
                diff = actions.diff,
                mark = actions.mark,
                implement = actions.implement,
                implementIn = actions.implementIn,
                changePlan = {
                    beside = null
                    focusAfterBeside++
                },
                openChanges = actions.openChanges,
            )
        }
        AgentBesideScreen(
            core = core,
            pcId = device.id,
            pcName = device.name,
            threadId = threadId,
            title = thread.title,
            status = status,
            agentName = agentName,
            running = running,
            pane = besideNow,
            onPane = { beside = it },
            changes = remember(running, open?.items, open?.turns) { AgentBeside.changes(running, open?.items.orEmpty(), open?.turns.orEmpty()) },
            plan = remember(thread.plan, open?.items) { AgentBeside.plan(thread.plan, open?.items.orEmpty()) },
            requests = waiting,
            buttonsFor = AgentBeside.withButtons(waiting, dock),
            onBack = { beside = null },
            onOpenEdit = { title, doc -> full = FullView.Diff(title, doc) },
            onOpenFile = { turn, path -> full = FullView.Changes(turn, path) },
            planCard = { itemId ->
                val unit = remember(rows, itemId) {
                    AgentPlanCards.units(rows).firstNotNullOfOrNull { (it as? RowUnit.Plan)?.takeIf { plan -> plan.head.itemId == itemId } }
                }
                if (unit != null) {
                    PlanCard(
                        unit,
                        RowsContext(itemsById, folder, newestPlan, running, implementing, thumbs, thread = thread, pcName = device.name, nowMs = now),
                        besideActions,
                    )
                } else {
                    Text(itemsById[itemId]?.text.orEmpty(), style = MaterialTheme.typography.bodyMedium)
                }
            },
            // The dock's own request or question, as a card: its Allow, Deny and answers, with its typing guard.
            dockCard = { request, position, total, maxHeight ->
                val shownAsk = queue?.takeIf { request is BesideRequest.Ask && it.shown.token == request.token }
                val shownQuestion = questionQueue?.takeIf { request is BesideRequest.Question && it.shown.token == request.token }
                if (shownAsk != null) {
                    RequestSheet(
                        queue = shownAsk.copy(position = position, total = total),
                        agentName = agentName,
                        folder = folder,
                        answering = answeringShown,
                        allowPrimary = true,
                        collapsed = false,
                        onToggle = {},
                        maxHeight = maxHeight,
                        onAnswer = answer,
                        onReview = { full = FullView.Edit(shownAsk.shown.token) },
                        card = true,
                    )
                } else if (shownQuestion != null && answers != null) {
                    QuestionDockSheet(
                        queue = shownQuestion.copy(position = position, total = total),
                        answers = answers,
                        agentName = agentName,
                        pcName = device.name,
                        replying = replyingShown,
                        collapsed = false,
                        onToggle = {},
                        maxHeight = maxHeight,
                        onChoose = { label -> answersState = QuestionDock.choose(shownQuestion.shown, answers, label) },
                        onChooseOther = {
                            // "Something else" is typed in the reply box: back to the thread for it.
                            answersState = QuestionDock.chooseOther(shownQuestion.shown, answers)
                            beside = null
                            focusAfterBeside++
                        },
                        onBack = { answersState = QuestionDock.back(answers) },
                        onNext = { answersState = QuestionDock.next(shownQuestion.shown, answers) },
                        onSend = sendQuestion,
                        onDismiss = dismissQuestion,
                        card = true,
                    )
                }
            },
            modifier = modifier,
        )
        link?.let { shown ->
            LinkSheet(
                link = shown,
                onOpen = {
                    link = null
                    openLink(context, shown.href)
                },
                onDismiss = { link = null },
            )
        }
        return
    }

    AgentsDrawer(
        device = device,
        core = core,
        control = drawer,
        selected = DrawerSelection.Thread(threadId),
        // Another thread or session replaces this one, as from its notification; Back still goes to the list.
        onOpenThread = { id -> AgentsNavigation.open(context, device.id, threadId = id) },
        onOpenSession = { id -> AgentsNavigation.open(context, device.id, sessionId = id) },
        modifier = modifier,
    ) {
        Column(Modifier.fillMaxSize().imePadding()) {
            ThreadHeader(
                thread = thread,
                status = status,
                agentName = agentName,
                openOn = if (openOnPc) device.name else null,
                onBack = onBack,
                canShare = thread != null && load == ThreadLoad.READY && !sharing,
                onShare = share,
                onDetails = { details = true },
                drawer = drawer,
                continueOn = if (!offline && load == ThreadLoad.READY) device.name else null,
                onContinue = handOver,
                onScripts = if (AgentScripts.menuShown(scriptsFolder, scriptRuns)) ({ scriptsSheet = true }) else null,
            )
            // "Kilele is showing this thread" · Show beside (board 10a): opens on what waits, else the changes.
            if (offerBeside && thread != null && load == ThreadLoad.READY && !offline) {
                BesideOfferLine(device.name) { beside = AgentBeside.firstPane(waiting = dock != null) }
            }
            if (AgentGit.stripShown(strip) && strip != null && !readonly) {
                GitStripLine(strip = strip, asOfMs = if (offline) gitSavedMs else null, now = now, onOpen = { gitSheet = true })
            }
            runningScript?.let { line ->
                ScriptRunningLine(
                    line = line,
                    onOpen = { scriptsSheet = true },
                    onStop = {
                        scope.launch {
                            if (core.agentScriptStop(device.id, line.run.id) is AgentCall.Failed) core.say(resources.getString(R.string.agents_failed_generic))
                        }
                    },
                )
            }
            // What this phone saved stands in while the PC can't be reached (or hasn't answered yet).
            val saved = open != null && open.loaded && open.savedMs != null
            if (offline && saved) OfflineBar(device.name, open?.savedMs, now)
            when {
                load == ThreadLoad.GONE || open?.removed == true -> AgentsEmptyState(title = stringResource(R.string.agents_thread_gone))
                load == ThreadLoad.UNREACHABLE && !saved -> AgentsEmptyState(
                    title = stringResource(R.string.agents_unreachable_title, device.name),
                    body = stringResource(R.string.agents_unreachable_body),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == ThreadLoad.NOT_ALLOWED -> AgentsEmptyState(
                    title = stringResource(R.string.agents_access_title, device.name),
                    body = stringResource(R.string.agents_access_body),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == ThreadLoad.UNAVAILABLE -> AgentsEmptyState(
                    title = stringResource(R.string.agents_unavailable, device.name),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                load == ThreadLoad.FAILED -> AgentsEmptyState(
                    title = stringResource(R.string.agents_failed_generic),
                    action = stringResource(R.string.agents_try_again),
                    onAction = retry,
                )
                // Loading: one line, no skeleton.
                open == null || !open.loaded || made == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(stringResource(R.string.agents_loading), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                open != null -> BoxWithConstraints(Modifier.fillMaxWidth().weight(1f)) {
                    // The request sheet rests at most at half the thread's height (reconcile file, the dock, item 4).
                    val sheetMax = maxHeight / 2
                    Column(Modifier.fillMaxSize()) {
                        val told = remember(device.id, thread, agents.catalog, pausedUntil) {
                            pausedUntil != null && AgentLimitAlarm.armedFor(context, device.id, thread.agent) != null
                        }
                        val end = threadEnd(status, thread, agents, device.name, told)
                        val heldRows: (LazyListScope.() -> Unit)? = if (heldHere.isEmpty()) {
                            null
                        } else {
                            {
                                items(heldHere.size, key = { "held:${heldHere[it].key}" }, contentType = { "held" }) { index ->
                                    val held = heldHere[index]
                                    HeldMessage(
                                        held = held,
                                        pcName = device.name,
                                        folder = folder,
                                        busy = held.key in heldBusy,
                                        onSend = { heldOp(held.key) { core.agentOutboxSend(device.id, held.key) } },
                                        onDiscard = { heldOp(held.key) { core.agentOutboxDiscard(device.id, held.key) } },
                                    )
                                }
                            }
                        }
                        AgentRowsList(
                            rows = rows,
                            context = remember(itemsById, folder, newestPlan, running, implementing, thumbs, thread, now, lit, undoing, agentName) {
                                RowsContext(
                                    itemsById,
                                    folder,
                                    newestPlan,
                                    running,
                                    implementing,
                                    thumbs,
                                    thread = thread,
                                    pcName = device.name,
                                    nowMs = now,
                                    highlightSeq = lit,
                                    undoing = undoing,
                                    agentName = agentName,
                                )
                            },
                            actions = actions,
                            earlier = open.moreBefore && open.items.isNotEmpty() && !offline,
                            loadingEarlier = loadingEarlier,
                            onLoadEarlier = loadEarlier,
                            listState = listState,
                            modifier = Modifier.weight(1f),
                            footer = if (end == null && heldRows == null) {
                                null
                            } else {
                                {
                                    end?.invoke(this)
                                    heldRows?.invoke(this)
                                }
                            },
                            scrollTo = jumpTo,
                            jump = jumps,
                        )
                        if (readonly) {
                            // Read only: no composer; the PC's folders decide (contract §U9.1 "Read-only thread").
                            Box(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp)) {
                                ComposerNote(stringResource(R.string.agents_readonly_add, folder), icon = G7Icons.folder)
                            }
                        } else Column(Modifier.fillMaxWidth().padding(horizontal = 10.dp)) {
                            val plan = thread?.plan
                            if (!plan.isNullOrEmpty()) TasksLine(plan, key = threadId)
                            // Pinned, never a row: the limit line above the queue, the dock and the reply box.
                            if (limit != null) LimitLine(limit)
                            if (control.queued.isNotEmpty()) {
                                QueueBox(
                                    control = control,
                                    agentName = agentName,
                                    waiting = thread?.state == AgentThreadState.WAITING || dock != null,
                                    busy = queueBusy,
                                    onSendNow = { id -> queueOp(id) { core.agentQueueSendNow(device.id, threadId, id) } },
                                    onEdit = { line -> editing = line },
                                    onRemove = { id -> queueOp(id) { core.agentQueueRemove(device.id, threadId, id) } },
                                    key = threadId,
                                )
                            }
                            if (questionQueue != null && answers != null) {
                                QuestionDockSheet(
                                    queue = questionQueue,
                                    answers = answers,
                                    agentName = agentName,
                                    pcName = device.name,
                                    replying = replyingShown,
                                    collapsed = sheetCollapsed,
                                    onToggle = { sheetCollapsed = !sheetCollapsed },
                                    maxHeight = sheetMax,
                                    onChoose = { label -> answersState = QuestionDock.choose(questionQueue.shown, answers, label) },
                                    onChooseOther = {
                                        answersState = QuestionDock.chooseOther(questionQueue.shown, answers)
                                        runCatching { focus.requestFocus() }
                                    },
                                    onBack = { answersState = QuestionDock.back(answers) },
                                    onNext = { answersState = QuestionDock.next(questionQueue.shown, answers) },
                                    onSend = sendQuestion,
                                    onDismiss = dismissQuestion,
                                )
                            }
                            if (queue != null) {
                                RequestSheet(
                                    queue = queue,
                                    agentName = agentName,
                                    folder = folder,
                                    answering = answeringShown,
                                    allowPrimary = composer.allowPrimary,
                                    collapsed = sheetCollapsed,
                                    onToggle = { sheetCollapsed = !sheetCollapsed },
                                    maxHeight = sheetMax,
                                    onAnswer = answer,
                                    onReview = { full = FullView.Edit(queue.shown.token) },
                                )
                            }
                            drafts.offer?.let { offer ->
                                DraftOfferLine(byName = offer.byName?.takeIf { it.isNotBlank() } ?: device.name) {
                                    val step = AgentDrafts.used(drafts)
                                    drafts = step.state
                                    step.text?.let { draft = it }
                                    step.images?.let { ids -> images = ids.map { PendingImage.Ready(it, it) } }
                                }
                            }
                            if (handoffLocked) ComposerNote(stringResource(R.string.agents_handoff_opens_unlocked, device.name), icon = G7Icons.lock)
                            if (AgentHandoffs.draftLine(handedOver != null, handoffDraft, draft)) DraftFromLine(device.name)
                            if (editingMessage && thread?.undo != null) EditingStrip(busy = undoing, onCancel = undo)
                            // Brought in and not yet sent to from G7: sending continues a copy (contract §U0.3).
                            if (thread?.found == true && open.turns.all { it.why == AgentCheckpointWhy.BEFORE }) {
                                ComposerNote(stringResource(R.string.agents_found_copy_note))
                            }
                            if (offline && images.isNotEmpty()) ComposerNote(stringResource(R.string.agents_pictures_wait, device.name), icon = G7Icons.photo, warn = true)
                            // A dev server the agent started (contract §V9.2): opened in the browser after the link sheet.
                            portLine?.let { line ->
                                PortLineView(line, device.name) { open ->
                                    // Asked again on the tap: the link may have changed since the line was made.
                                    scope.launch {
                                        val url = core.agentPortUrl(device.id, open.port.toUShort())?.takeIf(AgentPorts::opens)
                                        if (url != null) link = PendingLink(url, "")
                                    }
                                }
                            }
                            // Quote chips live in the draft's text, in front of the words (§SC9.1); a question's answer has none.
                            val (quotes, words) = remember(draft) { AgentQuotes.split(draft) }
                            val answering = question != null && answers != null
                            // The mic adds what was said to the box (or the question's own answer), selected, never sent (§V8.1).
                            val boxText = if (question != null && answers != null) QuestionDock.otherText(question, answers) else words
                            val dictation = rememberAgentDictation { heard ->
                                val limit = if (answering) QuestionDock.MAX_OTHER_BYTES else AgentsModel.MAX_PROMPT_CHARS
                                AgentVoice.appended(boxText, heard, limit)?.let { added ->
                                    if (answering) typed(added.text) else typed(AgentQuotes.typed(draft, added.text))
                                    composerSelection.select(added.start, added.end)
                                }
                            }
                            val canSpeak = dictation.available && composer.enabled && (!answering || questionField == QuestionField.OTHER)
                            if (canSpeak) AgentVoiceStrip(dictation)
                            val tapToTalk = rememberTapToTalk()
                            AgentComposer(
                                view = composer,
                                hint = if (dictation.holding) {
                                    stringResource(R.string.agents_voice_release)
                                } else {
                                    composerHint(composer.hint, agentName, pausedUntil, device.name)
                                },
                                voice = if (canSpeak) ({ AgentMicButton(dictation, tap = tapToTalk, enabled = !sending) }) else null,
                                selection = composerSelection,
                                text = boxText,
                                onText = { value -> if (answering) typed(value) else typed(AgentQuotes.typed(draft, value)) },
                                quotes = if (answering) emptyList() else quotes,
                                onRemoveQuote = { index ->
                                    draft = AgentQuotes.remove(draft, index)
                                    drafts = AgentDrafts.edited(drafts)
                                },
                                onFocus = focused,
                                focus = focus,
                                sending = sending,
                                stopping = stopping,
                                onStop = stop,
                                onSend = send,
                                joined = dock != null,
                                place = stringResource(R.string.agents_folder_on, folder, device.name),
                                agentName = agentName,
                                codex = codex,
                                settings = ComposerSettings(
                                    shown = abilities.control,
                                    model = AgentSettings.settingName(abilities.models, control.model, control.answeredModel),
                                    effort = AgentSettings.effectiveEffort(abilities.models, settingsNow)?.let(AgentSettings::effortLabel),
                                    mode = control.mode.takeIf { abilities.control },
                                ),
                                onSettings = openSettings,
                                onMode = openMode,
                                // Offline it shows off: pictures can't wait for the PC (board 22).
                                attachShown = abilities.control && imagesAllowed,
                                onAttach = { if (composer.attach) imageSheet = true },
                                images = images,
                                thumbs = thumbs,
                                onRemoveImage = { key -> images = AgentImages.removed(images, key) },
                                imagesRefused = questionField == null && images.isNotEmpty() && !imagesAllowed,
                                tooLong = questionField == null && AgentsModel.tooLong(draft),
                            )
                        }
                    }
                }
            }
        }
    }
    if (settingsOpen && thread != null) {
        AgentSettingsSheet(
            agentName = agentName,
            codex = codex,
            abilities = abilities,
            initial = settingsNow,
            appliesLater = true,
            onDone = configure,
            onDismiss = { settingsOpen = false },
        )
    }
    if (modeOpen && thread != null) {
        AgentModeSheet(
            title = stringResource(R.string.agents_settings_mode),
            modes = AgentModes.offered(abilities),
            codex = codex,
            selected = control.mode,
            note = stringResource(R.string.agents_model_applies),
            onPick = { mode -> configure(settingsNow.copy(mode = mode)) },
            onDismiss = { modeOpen = false },
        )
    }
    val implementItem = implementFor
    if (implementItem != null && thread != null) {
        val modes = AgentModes.implementModes(abilities)
        // Preselected as a new thread there would start: the folder's last mode for the agent, else Ask.
        val folderUse = remember(agents.catalog, thread.folder) {
            agents.catalog?.let { AgentModes.folderOf(NewThreadFfi.folders(it), thread.folder) }
        }
        AgentModeSheet(
            title = stringResource(R.string.agents_plan_implement),
            modes = modes,
            codex = codex,
            selected = AgentModes.preselect(folderUse, thread.agent, modes),
            onPick = { mode -> implementIn(implementItem, mode) },
            onDismiss = { implementFor = null },
        )
    }
    if (details && thread != null) {
        AgentDetailsSheet(thread = thread, control = control, abilities = abilities, pcName = device.name, now = now, onDismiss = { details = false })
    }
    if (imageSheet) {
        ImageSourceSheet(
            pcName = device.name,
            agentName = agentName,
            onCamera = {
                imageSheet = false
                pickers.camera()
            },
            onGallery = {
                imageSheet = false
                pickers.gallery(AgentImages.room(images))
            },
            onDismiss = { imageSheet = false },
        )
    }
    editing?.let { line ->
        QueueEditSheet(
            line = line,
            onSave = { text ->
                editing = null
                queueOp(line.id) { core.agentQueueEdit(device.id, threadId, line.id, text) }
            },
            onDismiss = { editing = null },
        )
    }
    link?.let { shown ->
        LinkSheet(
            link = shown,
            onOpen = {
                link = null
                openLink(context, shown.href)
            },
            onDismiss = { link = null },
        )
    }
    actionsFor?.let { row ->
        MessageActionsSheet(
            rows = AgentMessageActions.rows(row.acts, user = row.kind is AgentRowKind.User, offline = offline),
            agentName = agentName,
            pcName = device.name,
            onAction = { action -> onMessageAction(row, action) },
            onDismiss = { actionsFor = null },
        )
    }
    if (gitSheet) {
        AgentGitSheet(
            // Offline, the sheet is the strip this phone kept, every row needing the PC.
            status = if (offline) strip?.let { AgentGit.offlineStatus(it, gitSavedMs) } ?: gitStatus else gitStatus,
            prView = prView,
            refreshing = gitRefresh != null,
            context = GitSheetContext(
                pcName = device.name,
                agentName = agentName,
                offline = offline,
                pushAllowed = pushAllowed,
                threadBusy = threadBusy,
                asOfMs = if (offline) gitSavedMs else null,
                now = now,
            ),
            onReview = {
                gitSheet = false
                gitStatus?.let { full = FullView.Review(it) }
            },
            onPush = { push ->
                gitSheet = false
                pushAsk = push
            },
            onPr = {
                gitSheet = false
                gitStatus?.let { full = FullView.Pr(it) }
            },
            onBranch = {
                gitSheet = false
                branchSheet = true
            },
            onRefresh = gitRefreshNow,
            onDismiss = { gitSheet = false },
        )
    }
    pushAsk?.let { push ->
        PushConfirm(push = push, onPush = { remote -> startPush(push, remote) }, onDismiss = { pushAsk = null })
    }
    if (scriptsSheet) {
        AgentScriptsSheet(
            device = device,
            core = core,
            agents = agents,
            threadId = threadId,
            readonly = readonly,
            onOutput = { run ->
                scriptsSheet = false
                full = FullView.RunOutput(run)
            },
            onDismiss = { scriptsSheet = false },
        )
    }
    if (branchSheet) {
        gitStatus?.let { status ->
            AgentBranchSheet(
                device = device,
                core = core,
                threadId = threadId,
                status = status,
                threadBusy = threadBusy,
                agentName = agentName,
                onSwitched = { end ->
                    branchSheet = false
                    core.say(
                        when (end) {
                            is JobEnd.Switched -> resources.getString(if (end.created) R.string.agents_git_created else R.string.agents_git_switched, end.branch)
                            else -> resources.getString(R.string.agents_git_switched, status.branch.orEmpty())
                        },
                    )
                    gitRead++
                },
                onDismiss = { branchSheet = false },
            )
        }
    }
    rewind?.let { sheet ->
        val message = open?.items?.firstOrNull { it.turn == sheet.turn && it.kind == app.g7link.core.AgentItemKind.USER }?.text.orEmpty()
        RewindSheetView(
            sheet = sheet,
            message = message,
            agentName = agentName,
            pcName = device.name,
            nowMs = System.currentTimeMillis(),
            onRewind = applyRewind,
            onRetry = { openRewind(sheet.turn, sheet.refill) },
            onDismiss = { if (rewind !is RewindSheet.Applying) rewind = null },
        )
    }
}

/**
 * The first message on screen, for "Continue on <PC>" (contract §V8.5): the
 * PC opens the thread there. [earlier]: the "Earlier" row is the list's first.
 */
private fun firstSeqShown(listState: LazyListState, rows: List<AgentRow>, earlier: Boolean): ULong? {
    val units = AgentPlanCards.units(rows)
    val at = listState.firstVisibleItemIndex - if (earlier) 1 else 0
    val unit = units.getOrNull(at.coerceAtLeast(0)) ?: return null
    val seq = when (unit) {
        is RowUnit.One -> unit.row.seq
        is RowUnit.Plan -> unit.head.seq
    }
    return seq.takeIf { it > 0uL }
}

/** "Draft from Kilele" over the reply box (board 08b): the words came with the thread from the PC. */
@Composable
private fun DraftFromLine(pcName: String) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = 28.dp).padding(start = 8.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.monitor, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
        Text(stringResource(R.string.agents_handoff_draft_from, pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
    }
}

/**
 * The header: Back, the title (one line), its state in words with the
 * agent and folder under it, the drawer of threads, and More (Share). Stop is
 * in the reply box.
 */
@Composable
private fun ThreadHeader(
    thread: AgentThread?,
    status: ThreadStatus?,
    agentName: String,
    openOn: String?,
    onBack: () -> Unit,
    canShare: Boolean,
    onShare: () -> Unit,
    onDetails: () -> Unit,
    drawer: AgentsDrawerControl,
    /** "Continue on <PC>" (contract §V8.5): the PC's name, or null while it can't be offered. */
    continueOn: String? = null,
    onContinue: () -> Unit = {},
    /** Scripts (phase 6 §X5.2): shown once the folder has any or the thread has runs. */
    onScripts: (() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    var menu by remember { mutableStateOf(false) }
    Column {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 6.dp),
            verticalAlignment = Alignment.Top,
        ) {
            G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
            Column(Modifier.weight(1f).padding(start = 4.dp, top = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                // Two rows (owner's feedback, 2026-11-21): the title on one line, the state line under it.
                Text(
                    thread?.title.orEmpty(),
                    style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (thread != null && status != null) ThreadMeta(status, agentName, thread.folder, openOn)
            }
            AgentsDrawerButton(drawer)
            Box {
                G7IconButton(
                    icon = AgentGlyphs.more,
                    contentDescription = stringResource(R.string.agents_more),
                    onClick = { menu = true },
                    enabled = thread != null,
                )
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    if (continueOn != null) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.agents_handoff_continue_on, continueOn)) },
                            leadingIcon = { Icon(G7Icons.monitor, contentDescription = null, modifier = Modifier.size(18.dp)) },
                            onClick = {
                                menu = false
                                onContinue()
                            },
                        )
                    }
                    if (onScripts != null) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.agents_script_title)) },
                            onClick = {
                                menu = false
                                onScripts()
                            },
                        )
                    }
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.agents_details)) },
                        onClick = {
                            menu = false
                            onDetails()
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.agents_share)) },
                        enabled = canShare,
                        onClick = {
                            menu = false
                            onShare()
                        },
                    )
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
    }
}

/**
 * "● Needs you · Claude · g7linux-phone": the state in its colour, then the
 * agent and the folder's label, and "open on Kilele" while the PC shows the
 * thread (contract §V4.4).
 */
@Composable
private fun ThreadMeta(status: ThreadStatus, agentName: String, folder: String, openOn: String?) {
    val scheme = MaterialTheme.colorScheme
    val word = threadStateWord(status)
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        // The header's chip (§2.9): a mark of its own shape, then the word.
        ThreadStatusMark(status)
        StatusWord(status, word, MaterialTheme.typography.labelMedium.copy(fontWeight = FontWeight.SemiBold))
        val where = openOn?.let { stringResource(R.string.agents_open_on, it) }
        Text(
            listOfNotNull(agentName, folder, where).filter { it.isNotBlank() }.joinToString(" · ", prefix = "· "),
            style = MaterialTheme.typography.labelMedium,
            color = scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A thread's state in words, as its header says it. */
@Composable
internal fun threadStateWord(status: ThreadStatus): String = when (status) {
    ThreadStatus.NeedsYou -> stringResource(R.string.agents_state_waiting)
    ThreadStatus.Working -> stringResource(R.string.agents_state_working)
    ThreadStatus.Done -> stringResource(R.string.agents_state_done)
    ThreadStatus.PlanReady -> stringResource(R.string.agents_state_plan_ready)
    ThreadStatus.Stopped -> stringResource(R.string.agents_state_interrupted)
    is ThreadStatus.Paused -> stringResource(R.string.agents_state_paused)
    is ThreadStatus.Failed -> stringResource(R.string.agents_state_failed)
}

/**
 * How the thread ended when it ended badly, after its rows: "Failed" and the
 * CLI's own message as it is (with the sign-in command when it's signed out),
 * or the usage limit and when it resets in amber.
 */
@Composable
private fun threadEnd(status: ThreadStatus?, thread: AgentThread?, agents: AgentsState, pcName: String, told: Boolean): (LazyListScope.() -> Unit)? {
    if (thread == null) return null
    val context = LocalContext.current
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val colours = agentPalette()
    val fonts = LocalAppFonts.current
    return when (status) {
        is ThreadStatus.Failed -> {
            val login = if (status.login) {
                val hint = agents.catalog?.agents?.firstOrNull { it.id == thread.agent }?.loginHint?.takeIf { it.isNotBlank() }
                    ?: if (thread.agent == "codex") "codex login" else "claude auth login"
                stringResource(R.string.agents_login_failed, agents.agentName(thread.agent), hint, pcName)
            } else {
                null
            }
            val word = stringResource(R.string.agents_state_failed)
            val message = status.message
            val style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 12.5.sp, lineHeight = 19.sp)
            val loginStyle = MaterialTheme.typography.bodySmall
            val end: LazyListScope.() -> Unit = {
                item(key = "end", contentType = "end") {
                    Column(Modifier.fillMaxWidth().padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        EndLine(AgentGlyphs.warning, colours.statusFailed, word, colours.statusFailedText)
                        if (message != null) Text(message, style = style, color = scheme.onSurfaceVariant, modifier = Modifier.padding(start = 42.dp))
                        if (login != null) Text(login, style = loginStyle, modifier = Modifier.padding(start = 42.dp))
                    }
                }
            }
            end
        }
        is ThreadStatus.Paused -> {
            val at = status.resetsMs?.let { DateFormat.getTimeFormat(context).format(Date(it)) }
            // With the limit alarm set (contract §V5): the bell, and that you'll be told.
            val text = when {
                at != null && told -> stringResource(R.string.agents_paused_told, at)
                at != null -> stringResource(R.string.agents_paused_resets, at)
                else -> stringResource(R.string.agents_usage_limit)
            }
            val icon = if (at != null && told) G7Icons.bell else G7Icons.history
            val end: LazyListScope.() -> Unit = {
                item(key = "end", contentType = "end") {
                    Box(Modifier.padding(top = 12.dp)) { EndLine(icon, tints.amber, text, tints.amber) }
                }
            }
            end
        }
        else -> null
    }
}

@Composable
private fun EndLine(icon: ImageVector, tint: Color, text: String, color: Color) {
    Row(Modifier.fillMaxWidth().heightIn(min = 34.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.width(18.dp))
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(16.dp))
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium),
            color = color,
            modifier = Modifier.padding(start = 8.dp),
        )
    }
}

/**
 * The plan, pinned above the reply box (reconcile file, item 1): "Tasks 4/5 ·
 * <the step under way>"; it opens upward into the checklist and never scrolls
 * with the thread.
 */
@Composable
private fun TasksLine(plan: List<AgentPlanStep>, key: String) {
    val scheme = MaterialTheme.colorScheme
    var shown by rememberSaveable(key) { mutableStateOf(false) }
    val done = AgentsModel.planDone(plan)
    val current = AgentsModel.planPosition(plan)?.let { plan.getOrNull(it - 1)?.text }.orEmpty()
    Column(Modifier.fillMaxWidth().padding(bottom = 4.dp)) {
        AnimatedVisibility(visible = shown, enter = g7Expand(), exit = g7Collapse()) {
            val line = scheme.outlineVariant
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 240.dp)
                    .padding(start = 7.dp, bottom = 2.dp)
                    .drawBehind { drawLine(line, Offset(0f, 0f), Offset(0f, size.height), 1.dp.toPx()) }
                    .verticalScroll(rememberScrollState())
                    .padding(start = 15.dp),
            ) {
                for (step in plan) PlanStepRow(step)
            }
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 38.dp)
                .clip(RoundedCornerShape(6.dp))
                .clickable(role = Role.Button) { shown = !shown },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(14.dp), contentAlignment = Alignment.Center) {
                Icon(if (shown) G7Icons.chevronDown else AgentGlyphs.chevronUp, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
            }
            Box(Modifier.width(4.dp))
            Icon(G7Icons.clipboardList, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Text(
                stringResource(R.string.agents_tasks, done, plan.size),
                style = MaterialTheme.typography.bodyMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier.padding(start = 8.dp),
            )
            if (current.isNotBlank()) {
                Text(" · ", style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 1)
                Text(current, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun PlanStepRow(step: AgentPlanStep) {
    val scheme = MaterialTheme.colorScheme
    Row(Modifier.heightIn(min = 30.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        // Done steps a green check, the current one a blue ring (§2.9).
        Box(Modifier.size(14.dp), contentAlignment = Alignment.Center) {
            when (step.status) {
                AgentPlanStatus.COMPLETED -> ThreadStatusMark(ThreadStatus.Done, size = 12.dp)
                AgentPlanStatus.IN_PROGRESS -> ThreadStatusMark(ThreadStatus.Working, size = 10.dp)
                AgentPlanStatus.PENDING -> Box(Modifier.size(6.dp).border(1.5.dp, scheme.outline, CircleShape))
            }
        }
        Text(
            step.text,
            style = MaterialTheme.typography.bodyMedium.copy(fontWeight = if (step.status == AgentPlanStatus.IN_PROGRESS) FontWeight.Medium else FontWeight.Normal),
            color = if (step.status == AgentPlanStatus.IN_PROGRESS) scheme.onSurface else scheme.onSurfaceVariant,
        )
    }
}

/**
 * The request the thread waits on, as a sheet over the reply box (the phone
 * board, 4C): who wants what and "1 of 2", the agent's reason as its own words,
 * then the command with its folder or the change, scrolling between the title
 * and Deny and Allow, which stay in reach. Allow isn't offered for one the PC
 * cut; a held one shows with its buttons off. It never covers more than half
 * the thread and doesn't block scrolling it; its handle folds it to its first line.
 */
@Composable
private fun RequestSheet(
    queue: AskQueue,
    agentName: String,
    folder: String,
    answering: Boolean,
    allowPrimary: Boolean,
    collapsed: Boolean,
    onToggle: () -> Unit,
    maxHeight: Dp,
    onAnswer: (Boolean) -> Unit,
    onReview: () -> Unit,
    /** A card in the beside view's Requests (contract §V9.1, board 10d): no handle, edged all round. */
    card: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val fonts = LocalAppFonts.current
    val ask = queue.shown
    // A new request is a new sheet, scrolled to its top.
    key(ask.token) {
        val scroll = rememberScrollState()
        val doc = rememberDiff(if (ask.kind == AgentAskKind.EDIT) ask.diff else null)
        val body = remember(ask) { AgentsModel.askBody(ask) }
        val fitsHere = ask.kind != AgentAskKind.EDIT || ask.diff.isNullOrBlank() || (doc != null && !doc.more && diffRows(doc) <= SHEET_DIFF_ROWS)
        val shape = if (card) RoundedCornerShape(12.dp) else RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(max = maxHeight)
                .clip(shape)
                .background(scheme.surfaceContainer)
                .then(if (card) Modifier.border(1.dp, scheme.outlineVariant, shape) else Modifier)
                .padding(start = 14.dp, end = 14.dp, top = if (card) 12.dp else 0.dp, bottom = 14.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (!card) {
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(18.dp)
                        .clickable(role = Role.Button, onClick = onToggle),
                    contentAlignment = Alignment.BottomCenter,
                ) {
                    Box(Modifier.width(32.dp).height(4.dp).clip(RoundedCornerShape(2.dp)).background(scheme.outline))
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                val (icon, title) = when (ask.kind) {
                    AgentAskKind.COMMAND -> G7Icons.terminal to R.string.agents_dock_wants_command
                    AgentAskKind.EDIT -> G7Icons.edit to R.string.agents_dock_wants_edit
                    AgentAskKind.OTHER -> G7Icons.help to R.string.agents_dock_wants_other
                }
                Icon(icon, contentDescription = null, tint = tints.amber, modifier = Modifier.size(16.dp))
                Text(
                    stringResource(title, agentName),
                    style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (queue.total > 1) {
                    Text(
                        stringResource(R.string.agents_dock_count, queue.position, queue.total),
                        style = MaterialTheme.typography.labelMedium,
                        color = scheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
            }
            if (!collapsed) {
                Column(
                    modifier = Modifier.weight(1f, fill = false).verticalScroll(scroll),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    ask.reason?.takeIf { it.isNotBlank() }?.let { AgentSays(agentName, it) }
                    when {
                        ask.kind == AgentAskKind.EDIT && doc != null -> Column(
                            Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(6.dp))
                                .background(scheme.surface)
                                .clickable(role = Role.Button, onClick = onReview)
                                .padding(vertical = 8.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Row(Modifier.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                                FolderLabel(listOfNotNull(folder.ifBlank { null }, ask.path?.takeIf { it.isNotBlank() }).joinToString(" · "), Modifier.weight(1f))
                                Counts(doc.adds.toLong(), doc.dels.toLong())
                            }
                            DiffView(doc, wrap = true, maxRows = SHEET_DIFF_ROWS)
                            if (!fitsHere) SmallTextButton(stringResource(R.string.agents_show_all), onClick = onReview)
                        }
                        body != null -> Column(
                            Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(6.dp))
                                .background(scheme.surface)
                                .padding(horizontal = 12.dp, vertical = 10.dp),
                            verticalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            FolderLabel(folder)
                            Text(body, style = MaterialTheme.typography.bodySmall.mono(fonts).copy(fontSize = 13.sp, lineHeight = 19.5.sp))
                        }
                        else -> Text(ask.title, style = MaterialTheme.typography.bodyMedium)
                    }
                    if (ask.truncated) {
                        Text(stringResource(R.string.agents_ask_truncated), style = MaterialTheme.typography.bodySmall, color = tints.amber)
                    }
                }
                AnswerButtons(
                    queue = queue,
                    answering = answering,
                    allowPrimary = allowPrimary,
                    onAnswer = onAnswer,
                )
            }
        }
    }
}
