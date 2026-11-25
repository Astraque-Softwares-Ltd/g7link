// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.Preferences
import app.g7link.android.ui.agents.voice.AgentMicButton
import app.g7link.android.ui.agents.voice.AgentVoice
import app.g7link.android.ui.agents.voice.AgentVoiceStrip
import app.g7link.android.ui.agents.voice.rememberAgentDictation
import app.g7link.android.ui.agents.voice.rememberTapToTalk
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.g7Plate
import app.g7link.android.ui.theme.g7StateFraction
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentThread
import kotlinx.coroutines.launch

/**
 * A new thread, direction C (plans/new-thread-c-contract.md §5): a full
 * screen of three pages. Where: the folders used last as cards, the rest
 * behind "N more recent folders", and the PC's Home and disks to browse.
 * What: the task with a picture and the mic, Run with (up to three quick
 * picks from what each folder's newest thread ran), "Other agent, model or
 * thinking…", Asks before, Start. Browse: [AgentPlacesPage]. Back goes from
 * Browse and What to Where, and Where closes it, dropping the task as the
 * sheet did. The task, pictures, pick and mode stay while it's open.
 */
@Composable
internal fun NewThreadFlow(
    device: Device,
    core: Core,
    catalog: AgentCatalog,
    threads: List<AgentThread>,
    onStarted: (String) -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
    shared: SharedDraft? = null,
) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val folders = remember(catalog) { NewThreadFfi.folders(catalog) }
    val agents = remember(catalog) { NewThreadFfi.agents(catalog) }
    val folderIds = remember(folders) { folders.map { it.id } }
    // The folder a thread was last started in from this phone: a share lands there, and its card leads.
    val remembered = remember(device.id) { Preferences.agentChoice(context, device.id).second }
    var state by rememberSaveable(stateSaver = NEW_THREAD_SAVER) { mutableStateOf(NewThreadFlowModel.open(shared, remembered, folderIds)) }
    LaunchedEffect(folderIds) { state = NewThreadFlowModel.checked(state, folderIds) }
    val back: () -> Unit = { NewThreadFlowModel.back(state)?.let { state = it } ?: onClose() }
    BackHandler(onBack = back)

    // ---- Browsing the PC (Where's roots, the Browse page) ----
    val placesOffered = remember(device.id, device.online) { core.agentPlacesOffered(device.id) }
    var places by remember { mutableStateOf(PlacesState()) }
    val ask: (PlacesRequest) -> Unit = { request ->
        scope.launch {
            places = when (val answer = core.agentPlaces(device.id, request.place, request.offset)) {
                is PlacesAnswer.Ok -> AgentPlacesModel.listed(places, request, answer.value)
                is PlacesAnswer.Failed -> AgentPlacesModel.listFailed(places, request, answer.failure)
            }
        }
    }
    val openPlace: (String, String?) -> Unit = { place, folder ->
        val (next, request) = AgentPlacesModel.open(places, place, folder)
        places = next
        ask(request)
    }
    LaunchedEffect(placesOffered) {
        if (placesOffered && places.roots == null && !places.loading) openPlace("", null)
    }
    // "Approve on <PC>" while someone there is asked about the folder being added (and only that one).
    LaunchedEffect(device.id) {
        core.agentFolderPrompts.collect { prompt -> if (prompt.pcId == device.id) places = AgentPlacesModel.prompted(places, prompt.place) }
    }
    val use: (String, String?) -> Unit = { place, folder ->
        if (folder != null && folder in folderIds) {
            state = NewThreadFlowModel.choose(state, folder)
        } else {
            AgentPlacesModel.adding(places, place)?.let { (next, token) ->
                places = next
                scope.launch {
                    when (val answer = core.agentFolderAdd(device.id, place)) {
                        is PlacesAnswer.Ok -> {
                            places = AgentPlacesModel.added(places, token)
                            // Still on Browse: on to What in it. Gone back meanwhile: its card is there to pick.
                            if (state.step == FlowStep.BROWSE) state = NewThreadFlowModel.choose(state, answer.value.id)
                        }
                        is PlacesAnswer.Failed -> places = AgentPlacesModel.addFailed(places, token, answer.failure)
                    }
                }
            }
        }
    }

    // ---- What it runs with ----
    val folder = folders.firstOrNull { it.id == state.folderId }
    val picks = remember(folders, agents, folder?.id) { folder?.let { AgentSettings.quickPicks(folders, agents, it.id) }.orEmpty() }
    // A PC that doesn't take control starts every thread on the agent's defaults.
    val run = AgentSettings.runWith(picks, state.chosen)?.let { if (catalog.canControl) it else RunChoice(it.agent) }
    val agentInfo = catalog.agents.firstOrNull { it.id == run?.agent }
    val agentReady = agentInfo != null && agentInfo.installed && agentInfo.signedIn
    val abilities = remember(agentInfo, catalog.canControl) { AgentControlFfi.abilities(agentInfo, catalog.canControl) }
    val canPlan = abilities.can(AgentAbilities.PLAN)
    val mode = NewThreadFlowModel.mode(state, canPlan)
    val settings = run?.settings(mode) ?: ThreadSettings(mode = mode)

    // ---- Pictures: a share's, uploaded as the flow opens, and ones added here ----
    var images by remember { mutableStateOf(emptyList<PendingImage>()) }
    val thumbs = remember(device.id) { ImageThumbs { id -> (core.agentImageThumb(device.id, id) as? AgentCall.Ok)?.value } }
    LaunchedEffect(shared) {
        if (shared == null) return@LaunchedEffect
        if (shared.shortened) core.say(resources.getString(R.string.agents_share_shortened))
        for (path in shared.images) {
            images = AgentImages.adding(images, path)
            if (images.none { it.key == path }) {
                AgentImageIntake.deleteCameraFile(path)
                continue
            }
            val problem = when (val read = AgentImageIntake.read(context, android.net.Uri.fromFile(java.io.File(path)))) {
                is ImageRead.Problem -> read.problem
                is ImageRead.Bytes -> when (val sent = core.agentImageSend(device.id, read.name, read.bytes)) {
                    is AgentCall.Ok -> {
                        images = AgentImages.uploaded(images, path, sent.value)
                        null
                    }
                    is AgentCall.Failed -> ImageProblem.FAILED
                }
            }
            // The copy lives only until it's on the PC.
            AgentImageIntake.deleteCameraFile(path)
            if (problem != null) {
                images = AgentImages.removed(images, path)
                core.say(resources.getString(R.string.agents_image_failed))
            }
        }
    }
    val addImages: (List<android.net.Uri>, String?) -> Unit = { uris, cameraPath ->
        if (AgentImages.room(images) <= 0) AgentImageIntake.deleteCameraFile(cameraPath)
        uris.take(AgentImages.room(images)).forEachIndexed { index, uri ->
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
                // A photo taken for it lives only until it's on the PC.
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
    var imageSheet by remember { mutableStateOf(false) }
    var settingsOpen by remember { mutableStateOf(false) }

    // ---- Start ----
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val imagesTaken = images.isEmpty() || AgentSettings.takesImages(abilities, settings)
    val canStart = NewThreadFlowModel.canStart(busy, agentReady, folder != null, state.prompt, AgentImages.uploading(images), imagesTaken)
    val start: () -> Unit = start@{
        val agent = agentInfo ?: return@start
        val where = folder ?: return@start
        if (!canStart) return@start
        busy = true
        error = null
        scope.launch {
            when (val result = core.agentStart(device.id, agent.id, where.id, state.prompt.trim(), settings, AgentImages.readyIds(images))) {
                is AgentCall.Ok -> {
                    Preferences.setAgentChoice(context, device.id, agent.id, where.id)
                    onStarted(result.value.id)
                }
                is AgentCall.Failed -> error = when (result.failure) {
                    AgentFailure.DENIED -> resources.getString(R.string.agents_access_title, device.name)
                    AgentFailure.UNSUPPORTED -> resources.getString(R.string.agents_unavailable, device.name)
                    AgentFailure.OFFLINE, AgentFailure.TIMEOUT -> resources.getString(R.string.agents_unreachable_title, device.name)
                    AgentFailure.NOT_FOUND, AgentFailure.OTHER -> {
                        // The PC's BUSY reaches the phone as a plain failure: the caps (4 in all, 3 per agent) say which.
                        val running = threads.filter { AgentsModel.running(it) }
                        when {
                            running.size >= MAX_RUNNING -> resources.getString(R.string.agents_start_busy, device.name)
                            running.count { it.agent == agent.id } >= MAX_RUNNING_PER_AGENT ->
                                resources.getString(R.string.agents_start_busy_agent, device.name, agent.name)
                            else -> resources.getString(R.string.agents_failed_generic)
                        }
                    }
                }
            }
            busy = false
        }
    }

    val step = if (state.step == FlowStep.WHAT && folder == null) FlowStep.WHERE else state.step
    Column(modifier.fillMaxSize().imePadding()) {
        G7Header(title = stringResource(R.string.agents_new_thread), onBack = back, note = device.name)
        when (step) {
            FlowStep.WHERE -> WherePage(
                pcName = device.name,
                folders = folders,
                agents = agents,
                marked = state.folderId ?: remembered,
                shared = state.shared,
                placesOffered = placesOffered,
                asksFirst = !catalog.addFolders,
                places = places,
                onChoose = { id -> state = NewThreadFlowModel.choose(state, id) },
                onBrowse = { place ->
                    state = NewThreadFlowModel.browse(state)
                    if (place != places.place || places.failure != null || (places.entries.isEmpty() && !places.loading)) openPlace(place, null)
                },
            )
            FlowStep.BROWSE -> AgentPlacesPage(
                pcName = device.name,
                state = places,
                asksFirst = !catalog.addFolders,
                onOpen = openPlace,
                onMore = { AgentPlacesModel.more(places)?.let { (next, request) -> places = next; ask(request) } },
                onRetry = { AgentPlacesModel.retry(places).let { (next, request) -> places = next; ask(request) } },
                onUse = use,
                modifier = Modifier.weight(1f),
            )
            FlowStep.WHAT -> if (folder != null) {
                WhatPage(
                    pcName = device.name,
                    catalog = catalog,
                    folder = folder,
                    folders = folders,
                    agents = agents,
                    picks = picks,
                    chosen = state.chosen,
                    agentInfo = agentInfo,
                    abilities = abilities,
                    mode = mode,
                    prompt = state.prompt,
                    onPrompt = { state = NewThreadFlowModel.typed(state, it) },
                    images = images,
                    thumbs = thumbs,
                    imagesTaken = imagesTaken,
                    onRemoveImage = { key -> images = AgentImages.removed(images, key) },
                    onAddPicture = { imageSheet = true },
                    onPick = { choice -> state = NewThreadFlowModel.pick(state, choice) },
                    onOther = { settingsOpen = true },
                    onPlan = { on -> state = NewThreadFlowModel.plan(state, on) },
                    onWhere = { state = state.copy(step = FlowStep.WHERE) },
                    error = error,
                    busy = busy,
                    canStart = canStart,
                    onStart = start,
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }

    if (settingsOpen && run != null) {
        NewThreadSettingsSheet(
            agents = catalog.agents,
            control = catalog.canControl,
            initialAgent = run.agent,
            initial = run.settings(),
            onDone = { agentId, picked ->
                settingsOpen = false
                agents.firstOrNull { it.id == agentId }?.let { agent -> state = NewThreadFlowModel.pick(state, AgentSettings.choiceOf(agent, picked)) }
            },
            onDismiss = { settingsOpen = false },
        )
    }
    if (imageSheet) {
        ImageSourceSheet(
            pcName = device.name,
            agentName = agentInfo?.name.orEmpty(),
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
}

/** Where: "Where should it work?", the recent folders as cards, and the PC's Home and disks to browse. */
@Composable
private fun WherePage(
    pcName: String,
    folders: List<FolderUse>,
    agents: List<PickAgent>,
    marked: String?,
    shared: Boolean,
    placesOffered: Boolean,
    asksFirst: Boolean,
    places: PlacesState,
    onChoose: (String) -> Unit,
    onBrowse: (String) -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val now = rememberNow()
    // Cards don't move while the flow is open: the order is set as it opens, a folder added meanwhile comes first.
    var shownOrder by remember { mutableStateOf<List<String>?>(null) }
    val order = NewThreadFlowModel.order(folders, marked, shownOrder)
    if (order != shownOrder) SideEffect { shownOrder = order }
    val byId = folders.associateBy { it.id }
    var expanded by rememberSaveable { mutableStateOf(false) }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        OneLine(stringResource(R.string.agents_where_title), MaterialTheme.typography.titleLarge, scheme.onSurface, Modifier.padding(top = 4.dp, bottom = 4.dp))
        if (shared) Note(stringResource(R.string.agents_where_shared))
        val shown = if (expanded) order else order.take(NewThreadFlowModel.RECENT)
        for (id in shown) {
            val each = byId[id] ?: continue
            FolderCard(each, agents, marked = id == marked, now = now, onClick = { onChoose(id) })
        }
        val hidden = order.size - shown.size
        if (hidden > 0) {
            ChoiceRow(
                label = pluralStringResource(R.plurals.agents_where_more, hidden, hidden),
                role = Role.Button,
                onClick = { expanded = true },
            ) {
                Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
            }
        }
        if (placesOffered) {
            val roots = places.roots
            if (roots.isNullOrEmpty()) {
                // The roots aren't read yet (or couldn't be): Browse reads them, and says why it can't.
                ChoiceRow(label = stringResource(R.string.agents_where_all, pcName), icon = G7Icons.folder, role = Role.Button, onClick = { onBrowse("") }) {
                    Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                }
            } else {
                G7SectionLabel(stringResource(R.string.agents_where_all, pcName), Modifier.padding(top = 4.dp))
                roots.forEachIndexed { index, root ->
                    ChoiceRow(
                        label = rootName(root.name, index),
                        icon = rootIcon(root.name, index),
                        role = Role.Button,
                        onClick = { onBrowse(root.name) },
                    ) {
                        Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                    }
                }
            }
            if (asksFirst) Note(stringResource(R.string.agents_where_note, pcName))
        } else if (folders.isEmpty()) {
            // An older PC: folders are still added there.
            Note(stringResource(R.string.agents_no_folders, pcName))
        }
    }
}

/** A recent folder: its name and when it was used, the branch and the last thread's title, and what that thread ran with. */
@Composable
private fun FolderCard(folder: FolderUse, agents: List<PickAgent>, marked: Boolean, now: Long, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val shape = RoundedCornerShape(10.dp)
    val markedShown = g7StateFraction(marked)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .clip(shape)
            .border(1.dp, scheme.outlineVariant, shape)
            .g7Plate(markedShown, scheme.primary.copy(alpha = 0.10f), scheme.primary, shape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(G7Icons.folder, contentDescription = null, tint = tints.amber, modifier = Modifier.padding(top = 2.dp).size(20.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val last = folder.last
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OneLine(folder.label, MaterialTheme.typography.titleSmall, if (marked) scheme.primary else scheme.onSurface, Modifier.weight(1f))
                val time = when {
                    last == null -> stringResource(R.string.agents_where_not_used)
                    folder.usedMs > 0L -> relativeTime(folder.usedMs, now)
                    else -> null
                }
                if (time != null) OneLine(time, MaterialTheme.typography.labelSmall, scheme.onSurfaceVariant)
            }
            if (last != null) {
                val line = listOfNotNull(last.branch, last.title.ifBlank { null }).joinToString(" · ")
                if (line.isNotEmpty()) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        if (last.branch != null) Icon(AgentGlyphs.branch, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
                        OneLine(line, MaterialTheme.typography.bodySmall, scheme.onSurfaceVariant)
                    }
                }
                val agent = agents.firstOrNull { it.id == last.agent }
                val models = agent?.models.orEmpty()
                PickLabel(
                    agentId = last.agent,
                    agentName = agent?.name ?: last.agent.replaceFirstChar { it.uppercaseChar() },
                    modelName = AgentSettings.settingName(models, last.model),
                    effort = (last.effort ?: AgentSettings.model(models, last.model)?.defaultEffort)?.let(AgentSettings::effortLabel),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                )
            }
        }
    }
}

/** What: the folder, the task, Run with, Asks before and Start. */
@Composable
private fun WhatPage(
    pcName: String,
    catalog: AgentCatalog,
    folder: FolderUse,
    folders: List<FolderUse>,
    agents: List<PickAgent>,
    picks: List<QuickPick>,
    chosen: RunChoice?,
    agentInfo: AgentInfo?,
    abilities: AgentAbilities,
    mode: ThreadMode,
    prompt: String,
    onPrompt: (String) -> Unit,
    images: List<PendingImage>,
    thumbs: ImageThumbs,
    imagesTaken: Boolean,
    onRemoveImage: (String) -> Unit,
    onAddPicture: () -> Unit,
    onPick: (RunChoice) -> Unit,
    onOther: () -> Unit,
    onPlan: (Boolean) -> Unit,
    onWhere: () -> Unit,
    error: String?,
    busy: Boolean,
    canStart: Boolean,
    onStart: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth()) {
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            FolderHeader(folder, onClick = onWhere)
            // The mic adds what was said to the task, selected; Start is still the user's (contract §V8.1).
            val promptSelection = remember { TextSelection() }
            val dictation = rememberAgentDictation { heard ->
                AgentVoice.appended(prompt, heard, AgentsModel.MAX_PROMPT_CHARS)?.let { added ->
                    onPrompt(added.text)
                    promptSelection.select(added.start, added.end)
                }
            }
            AgentVoiceStrip(dictation)
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(scheme.surfaceContainer)) {
                AgentTextField(
                    value = prompt,
                    onValueChange = onPrompt,
                    hint = if (dictation.holding) stringResource(R.string.agents_voice_release) else stringResource(R.string.agents_prompt_hint),
                    minLines = 3,
                    maxLines = 8,
                    background = Color.Transparent,
                    selection = promptSelection,
                )
                val pictures = abilities.can(AgentAbilities.IMAGES)
                if (pictures || dictation.available) {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (pictures) {
                            G7IconButton(
                                icon = G7Icons.photo,
                                contentDescription = stringResource(R.string.agents_add_picture),
                                onClick = onAddPicture,
                                enabled = !busy && AgentImages.room(images) > 0,
                                tint = scheme.onSurfaceVariant,
                            )
                        }
                        Spacer(Modifier.weight(1f))
                        if (dictation.available) AgentMicButton(dictation, tap = rememberTapToTalk(), enabled = !busy)
                    }
                }
            }
            if (AgentsModel.tooLong(prompt)) {
                Text(stringResource(R.string.agents_too_long), style = MaterialTheme.typography.bodySmall, color = scheme.error)
            }
            if (images.isNotEmpty()) {
                ImageStrip(images = images, thumbs = thumbs, onRemove = onRemoveImage)
                if (!imagesTaken && agentInfo != null) {
                    Text(stringResource(R.string.agents_image_model, agentInfo.name), style = MaterialTheme.typography.bodySmall, color = scheme.error)
                }
            }
            if (catalog.canControl) {
                Column {
                    G7SectionLabel(stringResource(R.string.agents_run_with))
                    for (row in AgentSettings.runRows(picks, chosen)) {
                        PickRow(row, agents, onClick = { onPick(row.pick.choice) })
                    }
                    for (left in AgentSettings.leftOut(folders, agents)) {
                        catalog.agents.firstOrNull { it.id == left.id }?.let { NotReadyLine(it, pcName) }
                    }
                    ChoiceRow(
                        label = stringResource(R.string.agents_run_other),
                        icon = G7Icons.sparkle,
                        role = Role.Button,
                        onClick = onOther,
                    ) {
                        Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                    }
                }
            } else if (catalog.agents.size > 1) {
                // A PC without control: the agent alone, on its defaults.
                Column {
                    G7SectionLabel(stringResource(R.string.agents_pick_agent))
                    AgentChoiceRows(catalog.agents, selected = agentInfo?.id, onSelect = { id -> onPick(RunChoice(id)) })
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                G7SectionLabel(stringResource(R.string.agents_asks_before))
                val canPlan = abilities.can(AgentAbilities.PLAN)
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    G7RaisedButton(
                        label = stringResource(R.string.agents_asks_edits),
                        selected = mode == ThreadMode.SUPERVISED,
                        onClick = { onPlan(false) },
                        modifier = Modifier.weight(1f),
                    )
                    if (canPlan) {
                        G7RaisedButton(
                            label = stringResource(R.string.agents_asks_plan),
                            selected = mode == ThreadMode.PLAN,
                            onClick = { onPlan(true) },
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
                Text(
                    stringResource(if (mode == ThreadMode.PLAN) R.string.agents_mode_plan else R.string.agents_supervised_note),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 12.dp),
                )
                if (agentInfo?.id == "codex" && abilities.control && !canPlan) {
                    Text(
                        stringResource(R.string.agents_plan_unavailable),
                        style = MaterialTheme.typography.bodySmall,
                        color = scheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 12.dp),
                    )
                }
            }
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            error?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = scheme.error) }
            if (busy) {
                Box(Modifier.fillMaxWidth().heightIn(min = 48.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                }
            } else {
                G7PrimaryButton(label = stringResource(R.string.agents_start), enabled = canStart, onClick = onStart)
            }
        }
    }
}

/** What's header: the folder and the branch its newest thread was on; a tap goes back to Where. */
@Composable
private fun FolderHeader(folder: FolderUse, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(G7Icons.folder, contentDescription = null, tint = g7Tints().amber, modifier = Modifier.size(22.dp))
        Column(Modifier.weight(1f)) {
            OneLine(folder.label, MaterialTheme.typography.titleMedium, scheme.onSurface)
            val branch = folder.last?.branch
            if (branch != null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Icon(AgentGlyphs.branch, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
                    OneLine(branch, MaterialTheme.typography.bodySmall, scheme.onSurfaceVariant)
                }
            }
        }
        Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
    }
}

/** One row of Run with: the logo, "Claude · Opus 5 · High", and where it comes from. */
@Composable
private fun PickRow(row: RunRow, agents: List<PickAgent>, onClick: () -> Unit) {
    val choice = row.pick.choice
    val agent = agents.firstOrNull { it.id == choice.agent }
    val agentName = agent?.name ?: choice.agent.replaceFirstChar { it.uppercaseChar() }
    val models = agent?.models.orEmpty()
    val name = AgentSettings.pickName(models, choice)
    val delisted = AgentSettings.delisted(models, choice)
    val sub = when {
        delisted -> SubLine(stringResource(R.string.agents_pick_delisted, agentName), warn = true)
        row.pick.from == PickFrom.HERE -> SubLine(stringResource(R.string.agents_pick_here))
        row.pick.from == PickFrom.FOLDER -> SubLine(stringResource(R.string.agents_pick_used_in, row.pick.folder.orEmpty()))
        row.pick.from == PickFrom.YOURS -> SubLine(stringResource(R.string.agents_pick_yours))
        // "Claude's default" is already the label when no version is known.
        name != null -> SubLine(stringResource(R.string.agents_agent_default, agentName))
        else -> null
    }
    ChoiceRow(
        label = listOfNotNull(agentName, name).joinToString(" · "),
        lines = listOfNotNull(sub),
        selected = row.selected,
        onClick = onClick,
        title = { color ->
            PickLabel(
                agentId = choice.agent,
                agentName = agentName,
                modelName = name,
                effort = AgentSettings.pickEffort(models, choice),
                style = MaterialTheme.typography.bodyLarge,
                color = color,
                modifier = Modifier.weight(1f, fill = false),
            )
        },
    )
}

/** "Claude is signed out on <PC>." (and the command for it), or "isn't installed": why its pick isn't offered. */
@Composable
private fun NotReadyLine(agent: AgentInfo, pcName: String) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)) {
        val says = if (!agent.installed) {
            stringResource(R.string.agents_agent_missing_on, agent.name, pcName)
        } else {
            stringResource(R.string.agents_agent_signed_out_on, agent.name, pcName)
        }
        OneLine(says, MaterialTheme.typography.bodySmall, scheme.onSurfaceVariant)
        if (agent.installed && agent.loginHint.isNotBlank()) {
            OneLine(stringResource(R.string.agents_login_hint, agent.loginHint), MaterialTheme.typography.bodySmall, scheme.onSurfaceVariant)
        }
    }
}

/**
 * "<logo> Claude · Opus 5 · High" on one line: the model's name gives way
 * first, from its middle; with no version known, "<logo> Claude's default".
 */
@Composable
internal fun PickLabel(
    agentId: String,
    agentName: String,
    modelName: String?,
    effort: String?,
    style: TextStyle,
    color: Color,
    modifier: Modifier = Modifier,
) {
    val mark = AgentMark.of(agentId)
    val marks = agentMarkContent(mark)
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        if (modelName != null) {
            Text(markedText(agentName, mark), inlineContent = marks, style = style, color = color, maxLines = 1, softWrap = false)
            Text("·", style = style, color = color, maxLines = 1)
            Text(
                modelName,
                style = style,
                color = color,
                maxLines = 1,
                softWrap = false,
                overflow = TextOverflow.MiddleEllipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
        } else {
            Text(
                markedText(stringResource(R.string.agents_agent_default, agentName), mark),
                inlineContent = marks,
                style = style,
                color = color,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
        }
        if (effort != null) {
            Text("·", style = style, color = color, maxLines = 1)
            Text(effort, style = style, color = color, maxLines = 1, softWrap = false)
        }
    }
}

/** A line under a row's label: one line, amber when it warns. */
@Immutable
internal data class SubLine(val text: String, val warn: Boolean = false)

/**
 * A choice: an icon or an agent's logo, a label on one line (never wrapped),
 * lines under it, a small tag after it ("Default"), and [selected] drawn in
 * the accent. [title] replaces the label's text (a pick's three parts) and is
 * given the label's colour.
 */
@Composable
internal fun ChoiceRow(
    label: String,
    onClick: (() -> Unit)?,
    modifier: Modifier = Modifier,
    mark: AgentMark? = null,
    icon: ImageVector? = null,
    iconTint: Color = Color.Unspecified,
    lines: List<SubLine> = emptyList(),
    tag: String? = null,
    selected: Boolean = false,
    enabled: Boolean = true,
    role: Role = Role.RadioButton,
    title: (@Composable RowScope.(Color) -> Unit)? = null,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val shape = RoundedCornerShape(10.dp)
    val shown = g7StateFraction(selected)
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(shape)
            .g7Plate(shown, scheme.primary.copy(alpha = 0.14f), scheme.primary, shape)
            .then(if (onClick != null) Modifier.clickable(enabled = enabled, role = role, onClick = onClick) else Modifier)
            .semantics { if (selected) this.selected = true }
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        val fade = if (enabled) 1f else DISABLED_ALPHA
        if (icon != null) {
            Icon(icon, contentDescription = null, tint = (if (iconTint == Color.Unspecified) scheme.onSurfaceVariant else iconTint).copy(alpha = fade), modifier = Modifier.size(22.dp))
        }
        Column(Modifier.weight(1f)) {
            val labelColor = (if (selected) scheme.primary else scheme.onSurface).copy(alpha = fade)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (title != null) {
                    title(labelColor)
                } else {
                    Text(
                        markedText(label, mark),
                        inlineContent = agentMarkContent(mark),
                        style = MaterialTheme.typography.bodyLarge,
                        color = labelColor,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
                if (tag != null) OneLine(tag, MaterialTheme.typography.labelSmall, scheme.onSurfaceVariant.copy(alpha = fade))
            }
            for (line in lines) {
                OneLine(line.text, MaterialTheme.typography.bodySmall, (if (line.warn) tints.amber else scheme.onSurfaceVariant).copy(alpha = fade))
            }
        }
        trailing?.invoke(this)
    }
}

/** The PC's agents, each with its logo and whether it can run: signed in, signed out (and the command for it), not installed. */
@Composable
internal fun AgentChoiceRows(agents: List<AgentInfo>, selected: String?, onSelect: (String) -> Unit) {
    for (agent in agents) {
        val ready = agent.installed && agent.signedIn
        val lines = when {
            !agent.installed -> listOf(SubLine(stringResource(R.string.agents_not_installed)))
            !agent.signedIn -> listOf(
                SubLine(stringResource(R.string.agents_signed_out)),
                SubLine(stringResource(R.string.agents_login_hint, agent.loginHint)),
            )
            agent.auth == "api_key" -> listOf(SubLine(stringResource(R.string.agents_api_key)))
            else -> listOf(SubLine(stringResource(R.string.agents_signed_in)))
        }
        ChoiceRow(
            label = agent.name,
            mark = AgentMark.of(agent.id),
            lines = lines,
            enabled = ready,
            selected = ready && agent.id == selected,
            onClick = if (ready) ({ onSelect(agent.id) }) else null,
        )
    }
}

/** A root's name as shown: the first, "Home", is the user's home; a disk keeps its own name. */
@Composable
internal fun rootName(name: String, index: Int): String =
    if (isHome(name, index)) stringResource(R.string.agents_places_home) else name

internal fun rootIcon(name: String, index: Int): ImageVector = if (isHome(name, index)) G7Icons.home else AgentGlyphs.server

private fun isHome(name: String, index: Int): Boolean = index == 0 && name == HOME_ROOT

/** One line of text, cut short at its end. */
@Composable
internal fun OneLine(text: String, style: TextStyle, color: Color, modifier: Modifier = Modifier) {
    Text(text, style = style, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = modifier)
}

@Composable
private fun Note(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
    )
}

/** The flow kept across a rotation or the app going to the background. */
private val NEW_THREAD_SAVER: Saver<NewThreadState, Any> = listSaver<NewThreadState, Any?>(
    save = { s ->
        listOf(s.step.name, s.folderId, s.prompt, s.chosen?.agent, s.chosen?.model, s.chosen?.effort, s.chosen?.custom == true, s.plan, s.shared)
    },
    restore = { l ->
        NewThreadState(
            step = FlowStep.entries.firstOrNull { it.name == l[0] } ?: FlowStep.WHERE,
            folderId = l[1] as String?,
            prompt = l[2] as String? ?: "",
            chosen = (l[3] as String?)?.let { agent -> RunChoice(agent, l[4] as String?, l[5] as String?, l[6] as Boolean? == true) },
            plan = l[7] as Boolean? == true,
            shared = l[8] as Boolean? == true,
        )
    },
)

/** The root the PC lists first: its user's home. */
private const val HOME_ROOT = "Home"

private const val DISABLED_ALPHA = 0.45f

/** The PC's limits on running threads (the reconcile file): 4 in all, 3 per agent. */
private const val MAX_RUNNING = 4
private const val MAX_RUNNING_PER_AGENT = 3
