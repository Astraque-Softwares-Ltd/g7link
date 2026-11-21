// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Keycap
import app.g7link.android.ui.theme.G7KeycapSurface
import app.g7link.android.ui.theme.G7Well
import app.g7link.android.ui.theme.g7Tints
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The typing input the Touchpad and Screen tabs share, while its tab shows:
 * the box's text, what it knows of the PC's focused field, the keys held for
 * the next key, and the pictures and files waiting for the next Send.
 */
@Stable
internal class TypingInputState(
    val pcId: String,
    val typing: TypingSync,
    private val scope: CoroutineScope,
    attachmentsState: MutableState<List<Attachment>>,
    val pickImage: () -> Unit,
    val pickFiles: () -> Unit,
) {
    var typed by mutableStateOf(TextFieldValue(""))
    var pcField by mutableStateOf<PcField?>(null)
        private set
    var pcUnverified by mutableStateOf(false)
        private set
    var attachments by attachmentsState
    val activeMods = mutableStateListOf<String>()
    val sendTyping = Channel<Unit>(Channel.CONFLATED)
    val focusRequester = FocusRequester()

    /** The bubble follows what [typing] knows of the PC's field. */
    fun showField() {
        pcField = typing.current
        pcUnverified = typing.unverified
    }

    /** A click or key from here moved the PC's caret: what's in the box may no longer match. */
    fun caretMoved() {
        typing.onPcCaretMoved()
        showField()
    }

    /** Waits (briefly) until everything typed has reached the PC, then runs [then]. */
    fun afterTyping(then: () -> Unit) {
        scope.launch {
            withTimeoutOrNull(2_000) {
                while (!typing.synced) {
                    sendTyping.trySend(Unit)
                    delay(20)
                }
            }
            then()
        }
    }

    fun launch(block: suspend CoroutineScope.() -> Unit) {
        scope.launch(block = block)
    }
}

/**
 * The typing input's state for [pcId], kept while the tab shows. While
 * [showKeyboard], the PC tells of its focused field, and the box takes the
 * focus and opens the keyboard.
 */
@Composable
internal fun rememberTypingInput(
    pcId: String,
    core: Core,
    showKeyboard: Boolean,
    onStatus: (Core.RemoteAccess) -> Unit,
): TypingInputState {
    val scope = rememberCoroutineScope()
    // Pictures and files picked wait over the box, to go with the next Send.
    val attachments = rememberSaveable(pcId, stateSaver = Attachment.listSaver) {
        mutableStateOf(emptyList<Attachment>())
    }
    val appContext = LocalContext.current.applicationContext
    val attach: (List<Uri>) -> Unit = { uris ->
        scope.launch {
            val picked = withContext(Dispatchers.IO) { uris.mapNotNull { Attachment.describe(appContext, it) } }
            val held = attachments.value
            attachments.value = (held + picked.filter { new -> held.none { it.uri == new.uri } }).take(Attachment.MAX)
        }
    }
    val pickImage = rememberLauncherForActivityResult(
        ActivityResultContracts.PickMultipleVisualMedia(Attachment.MAX),
    ) { uris -> if (uris.isNotEmpty()) attach(uris) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) attach(uris)
    }
    // The typing box: a real field, so the keyboard keeps its page, autocorrects
    // and has something to delete. It follows the PC's focused field: that
    // field's text where the PC shares it, or what was typed here into it.
    val state = remember(pcId) {
        TypingInputState(
            pcId = pcId,
            typing = TypingSync(FieldDrafts.of(pcId)),
            scope = scope,
            attachmentsState = attachments,
            pickImage = { pickImage.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
            pickFiles = { pickFiles.launch(arrayOf("*/*")) },
        )
    }
    val typing = state.typing
    DisposableEffect(typing) {
        typing.onReplace = { box -> state.typed = TextFieldValue(box.text, TextRange(box.selStart, box.selEnd)) }
        onDispose { typing.onReplace = null }
    }
    // While the box is open, the PC tells of its focused field.
    DisposableEffect(pcId, showKeyboard) {
        if (showKeyboard) core.remoteTextWatch(pcId, true)
        onDispose {
            if (showKeyboard) {
                core.remoteTextWatch(pcId, false)
                typing.close()
                state.showField()
            }
        }
    }
    // The PC sends the field on the path the box last asked on: after a switch, it asks again.
    val startPath = remember(core, pcId) { core.state.value.device(pcId)?.activePath }
    val activePath by remember(core, pcId) {
        core.state.map { it.device(pcId)?.activePath }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(initialValue = startPath)
    var watchedPath by remember(pcId) { mutableStateOf(activePath) }
    LaunchedEffect(showKeyboard, activePath) {
        if (watchAgain(showKeyboard, activePath, watchedPath)) core.remoteTextWatch(pcId, true)
        watchedPath = activePath
    }
    val pcState by remember(core, pcId) {
        core.state.map { it.pcTextState[pcId] }.distinctUntilChanged()
    }.collectAsStateWithLifecycle(initialValue = null)
    LaunchedEffect(typing, pcState) {
        val field = pcState ?: return@LaunchedEffect
        typing.onState(field.toPcField())
        state.showField()
        state.sendTyping.trySend(Unit)
    }
    // One change at a time, each worked out from what the PC has.
    LaunchedEffect(typing) {
        for (wake in state.sendTyping) {
            val op = typing.nextOp() ?: continue
            core.remoteTyping(pcId, op, onStatus) { result ->
                typing.onDone(op, result)
                state.sendTyping.trySend(Unit)
            }
        }
    }
    // The PC's own changes come in once both sides are quiet.
    LaunchedEffect(typing) {
        while (true) {
            delay(100)
            if (typing.tick()) state.sendTyping.trySend(Unit)
        }
    }
    // A click or key from here moved the PC's caret: what's in the box may no longer match.
    LaunchedEffect(core, typing) {
        core.pcCaretMoved.collect { id -> if (id == pcId) state.caretMoved() }
    }
    val keyboardController = LocalSoftwareKeyboardController.current
    LaunchedEffect(showKeyboard) {
        if (showKeyboard) {
            runCatching { state.focusRequester.requestFocus() }
            keyboardController?.show()
        } else {
            keyboardController?.hide()
        }
    }
    return state
}

/**
 * Typing into the PC, as the Touchpad and Screen tabs both have it: the
 * special keys and modifiers, then the composer (what's typed appears on the
 * PC as it's typed, the phone's Enter is a new line or loads a link, and the
 * round button sends or dictates).
 */
@Composable
internal fun TypingInput(
    state: TypingInputState,
    pcName: String,
    core: Core,
    utteranceJoiner: UtteranceJoiner,
    onStatus: (Core.RemoteAccess) -> Unit,
    pasteReady: Boolean,
    onPasted: () -> Unit,
    onTypedHere: () -> Unit,
    haptics: (TrackpadHaptic) -> Unit,
    modifier: Modifier = Modifier,
    spacing: Dp = 10.dp,
) {
    val pcId = state.pcId
    val typing = state.typing
    val activeMods = state.activeMods
    Column(modifier = modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(spacing)) {
        SpecialKeysBar(
            activeMods = activeMods,
            showExtraKeys = true,
            pasteReady = pasteReady,
            onToggleMod = { mod ->
                haptics(TrackpadHaptic.Key)
                if (mod in activeMods) activeMods.remove(mod) else activeMods.add(mod)
            },
            onKey = { key, forceMods ->
                haptics(TrackpadHaptic.Key)
                val mods = forceMods ?: activeMods.toList()
                core.remoteKey(pcId, key, mods, onStatus)
                if (forceMods == null) activeMods.clear()
                if (key == "paste") onPasted()
            },
        )

        TypingComposer(
            pcId = pcId,
            pcName = pcName,
            core = core,
            utteranceJoiner = utteranceJoiner,
            onStatus = onStatus,
            typed = state.typed,
            field = state.pcField,
            unverified = state.pcUnverified,
            onStartAfresh = {
                typing.startAfresh()
                state.showField()
            },
            onTyped = { next ->
                val typed = state.typed
                val diff = typingDiff(typed.text, next.text)
                if (activeMods.isNotEmpty() && diff.backspaces == 0 && diff.added.length == 1 &&
                    diff.added[0].isLetterOrDigit()
                ) {
                    // A shortcut (Ctrl+C...), not text: it stays out of the field.
                    core.remoteKey(pcId, diff.added.lowercase(), activeMods.toList(), onStatus)
                    activeMods.clear()
                    return@TypingComposer
                }
                // Enter after a link, or with nothing typed here, loads it: a plain
                // Enter, as Shift+Enter opens a browser's address in a new window.
                if (diff.backspaces == 0 && diff.added == "\n" && next.text == typed.text + "\n" &&
                    typing.kind != FieldKind.Secret && enterGoesAsIs(typed.text)
                ) {
                    val mods = activeMods.toList()
                    activeMods.clear()
                    state.afterTyping {
                        core.remoteKey(pcId, "enter", mods, onStatus)
                        typing.onSent()
                        state.showField()
                    }
                    return@TypingComposer
                }
                // Modifiers are for shortcuts and keys: typing lets go of them.
                if (next.text != typed.text) {
                    activeMods.clear()
                    onTypedHere()
                }
                // What changed goes over as one edit where it changed (a new
                // line is Shift+Enter: a line break in chats and prompts).
                state.typed = next
                typing.onLocal(
                    BoxText(next.text, next.selection.start, next.selection.end, next.composition != null),
                )
                state.sendTyping.trySend(Unit)
            },
            attachments = state.attachments,
            onRemoveAttachment = { gone -> state.attachments = state.attachments.filterNot { it.uri == gone.uri } },
            onSend = {
                val mods = activeMods.toList()
                activeMods.clear()
                val sending = state.attachments
                state.attachments = emptyList()
                // Pictures paste where the PC is typing, files go to it, then
                // Enter sends what's typed (with nothing typed, no Enter).
                state.afterTyping {
                    state.launch {
                        val failed = mutableListOf<Attachment>()
                        val files = mutableListOf<Uri>()
                        for (item in sending) {
                            val uri = Uri.parse(item.uri)
                            if (!item.isImage) {
                                files += uri
                            } else if (!core.pasteImageOnPc(pcId, uri, onStatus)) {
                                failed += item
                            } else {
                                delay(PASTE_SETTLE_MS)
                            }
                        }
                        if (files.isNotEmpty()) core.sendFiles(pcId, files)
                        state.attachments = failed + state.attachments
                        if (state.typed.text.isNotEmpty() && failed.isEmpty()) {
                            core.remoteKey(pcId, "enter", mods, onStatus)
                            typing.onSent()
                            state.showField()
                        }
                    }
                }
            },
            onBackspaceWhenEmpty = {
                core.remoteKey(pcId, "backspace", activeMods.toList(), onStatus)
                activeMods.clear()
            },
            onPickImage = state.pickImage,
            onPickFiles = state.pickFiles,
            focusRequester = state.focusRequester,
        )
    }
}

/** A moment after a picture's paste key, for the app to take it before the next. */
private const val PASTE_SETTLE_MS = 300L

/** A special key with its edge: a 40 dp face over a 4 dp edge, 44 dp to touch. */
private val SPECIAL_KEY_HEIGHT = 44.dp

/**
 * Typing into the PC the way a chat app types: a round field that grows to
 * five lines, a paperclip (send files) and a picture (paste an image) inside
 * it, and a round button beside it. With text it sends (Enter on the PC) and
 * clears the field; empty it dictates: a tap starts and the next one stops, or
 * hold to talk, let go to type it and slide left to drop it, as a voice note.
 */
@Composable
private fun TypingComposer(
    pcId: String,
    pcName: String,
    core: Core,
    utteranceJoiner: UtteranceJoiner,
    onStatus: (Core.RemoteAccess) -> Unit,
    typed: TextFieldValue,
    field: PcField?,
    unverified: Boolean,
    onStartAfresh: () -> Unit,
    attachments: List<Attachment>,
    onRemoveAttachment: (Attachment) -> Unit,
    onTyped: (TextFieldValue) -> Unit,
    onSend: () -> Unit,
    onBackspaceWhenEmpty: () -> Unit,
    onPickImage: () -> Unit,
    onPickFiles: () -> Unit,
    focusRequester: FocusRequester,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val dictation = rememberDictation(pcId, core, utteranceJoiner, onStatus)
    val latest by rememberUpdatedState(dictation)
    val state = dictation.state
    val listening = state is DictateState.Listening
    val sends = (typed.text.isNotEmpty() || attachments.isNotEmpty()) && !listening
    val secret = field?.kind == FieldKind.Secret
    val currentSends by rememberUpdatedState(sends)
    val currentOnSend by rememberUpdatedState(onSend)
    // Held down to talk, and dragged far enough left to drop what's heard.
    var holding by remember { mutableStateOf(false) }
    var dropping by remember { mutableStateOf(false) }
    // The round key goes down under the finger, as the keycaps do.
    var buttonDown by remember { mutableStateOf(false) }
    val placeholder = stringResource(R.string.remote_type_placeholder, pcName)
    val pillShape = RoundedCornerShape(24.dp)

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        FieldBubble(
            field = field,
            pcName = pcName,
            unverified = unverified,
            onStartAfresh = onStartAfresh,
            loadIcon = { key -> core.pcAppIcon(pcId, key) },
        )
        if (attachments.isNotEmpty()) {
            AttachmentTray(attachments, onRemoveAttachment, Modifier.padding(bottom = 4.dp))
        }
        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (listening) {
                // Like a voice note: the field gives way to what's heard.
                G7Well(modifier = Modifier.weight(1f), shape = pillShape) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(min = 48.dp)
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Box(Modifier.size(10.dp).clip(CircleShape).background(tints.red))
                        Text(
                            (state as? DictateState.Listening)?.partial?.ifEmpty { null }
                                ?: stringResource(R.string.remote_dictate_listening, pcName),
                            style = MaterialTheme.typography.bodyMedium,
                            color = if (dropping) scheme.onSurfaceVariant else scheme.onSurface,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            } else {
                G7Well(modifier = Modifier.weight(1f), shape = pillShape) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(min = 48.dp)
                            .padding(start = 16.dp, end = 2.dp),
                        verticalAlignment = Alignment.Bottom,
                    ) {
                        BasicTextField(
                            value = typed,
                            onValueChange = onTyped,
                            textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onSurface),
                            cursorBrush = SolidColor(scheme.primary),
                            maxLines = 5,
                            // Enter is a new line here, as in a chat app; the round button sends.
                            // A password goes through unseen, without suggestions.
                            keyboardOptions = if (secret) {
                                KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Default)
                            } else {
                                KeyboardOptions(
                                    capitalization = KeyboardCapitalization.Sentences,
                                    imeAction = ImeAction.Default,
                                )
                            },
                            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
                            modifier = Modifier
                                .weight(1f)
                                .padding(vertical = 13.dp)
                                .focusRequester(focusRequester)
                                // Backspace with nothing typed here still deletes on the PC.
                                .onPreviewKeyEvent { event ->
                                    if (event.type == KeyEventType.KeyDown && event.key == Key.Backspace &&
                                        typed.text.isEmpty()
                                    ) {
                                        onBackspaceWhenEmpty()
                                        true
                                    } else {
                                        false
                                    }
                                }
                                .semantics { contentDescription = placeholder },
                            decorationBox = { field ->
                                Box {
                                    if (typed.text.isEmpty()) {
                                        Text(
                                            placeholder,
                                            style = MaterialTheme.typography.bodyLarge,
                                            color = scheme.onSurfaceVariant,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis,
                                        )
                                    }
                                    field()
                                }
                            },
                        )
                        IconButton(onClick = onPickFiles) {
                            Icon(
                                G7Icons.paperclip,
                                contentDescription = stringResource(R.string.remote_send_files, pcName),
                                tint = scheme.onSurfaceVariant,
                                modifier = Modifier.size(20.dp),
                            )
                        }
                        IconButton(onClick = onPickImage) {
                            Icon(
                                G7Icons.photo,
                                contentDescription = stringResource(R.string.remote_paste_image, pcName),
                                tint = scheme.onSurfaceVariant,
                                modifier = Modifier.size(20.dp),
                            )
                        }
                    }
                }
            }

            val buttonLabel = stringResource(
                when {
                    sends -> R.string.remote_send
                    listening -> R.string.remote_dictate_stop
                    else -> R.string.remote_dictate
                },
            )
            G7KeycapSurface(
                pressed = buttonDown,
                face = if (listening) tints.red else scheme.primary,
                shape = CircleShape,
                edge = SEND_KEY_EDGE,
                modifier = Modifier
                    .width(48.dp)
                    .height(48.dp + SEND_KEY_EDGE)
                    .semantics(mergeDescendants = true) {
                        role = Role.Button
                        contentDescription = buttonLabel
                        onClick(label = buttonLabel) {
                            when {
                                currentSends -> currentOnSend()
                                latest.state is DictateState.Listening -> latest.stop()
                                else -> latest.start()
                            }
                            true
                        }
                    }
                    .pointerInput(pcId) {
                        val holdMs = 350L
                        val dropPx = 80.dp.toPx()
                        awaitEachGesture {
                            val down = awaitFirstDown(requireUnconsumed = false)
                            val downTime = System.currentTimeMillis()
                            buttonDown = true
                            try {
                                if (currentSends) {
                                    waitForUpOrCancellation()?.let { currentOnSend() }
                                    return@awaitEachGesture
                                }
                                if (latest.state is DictateState.Listening) {
                                    waitForUpOrCancellation()
                                    latest.stop()
                                    return@awaitEachGesture
                                }
                                latest.start()
                                while (true) {
                                    val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id }
                                    if (change == null || !change.pressed) break
                                    if (System.currentTimeMillis() - downTime >= holdMs) holding = true
                                    dropping = holding && change.position.x - down.position.x < -dropPx
                                }
                                // A tap leaves it listening until the next tap; a hold ends on lifting.
                                if (holding) {
                                    if (dropping) latest.cancel() else latest.stop()
                                }
                                holding = false
                                dropping = false
                            } finally {
                                buttonDown = false
                            }
                        }
                    },
            ) {
                Icon(
                    if (sends) G7Icons.arrowUp else G7Icons.mic,
                    contentDescription = null,
                    tint = if (listening) scheme.surface else scheme.onPrimary,
                    modifier = Modifier.size(22.dp),
                )
            }
        }

        // What dictation is doing, under the field, only while there's something to say.
        val status = when (val s = state) {
            is DictateState.Listening -> stringResource(
                if (holding) R.string.remote_dictate_hold_hint else R.string.remote_dictate_tap_hint,
            )
            is DictateState.Typing -> stringResource(R.string.remote_dictate_typing, pcName)
            is DictateState.Error -> when (s.kind) {
                DictateError.NoRecognizer -> stringResource(R.string.remote_dictate_no_recognizer)
                DictateError.PermissionNeeded -> stringResource(R.string.remote_dictate_permission_needed, pcName)
                DictateError.NoSpeech -> stringResource(R.string.remote_dictate_no_speech)
                DictateError.Failed -> stringResource(R.string.remote_dictate_error)
            }
            is DictateState.Idle -> null
        }
        if (status != null) {
            Text(
                status,
                style = MaterialTheme.typography.bodySmall,
                color = if (state is DictateState.Error) tints.red else scheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 16.dp),
            )
        }
    }
}

@Composable
private fun SpecialKeysBar(
    activeMods: List<String>,
    showExtraKeys: Boolean,
    pasteReady: Boolean,
    onToggleMod: (String) -> Unit,
    onKey: (key: String, forceMods: List<String>?) -> Unit,
) {
    val keyModifier = Modifier.height(SPECIAL_KEY_HEIGHT)
    val keyStyle = MaterialTheme.typography.labelMedium
    val keyPadding = PaddingValues(horizontal = 2.dp)
    Column(
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        // Modifier keys row: 4 equal columns that fit at every font scale without overflowing
        val mods = listOf(
            "ctrl" to stringResource(R.string.remote_key_ctrl),
            "alt" to stringResource(R.string.remote_key_alt),
            "shift" to stringResource(R.string.remote_key_shift),
            "win" to stringResource(R.string.remote_key_win),
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for ((id, label) in mods) {
                // A held modifier goes with the next key: latched down in the accent so it's never missed.
                G7Keycap(
                    label = label,
                    onClick = { onToggleMod(id) },
                    selected = id in activeMods,
                    corner = SPECIAL_KEY_CORNER,
                    edge = SPECIAL_KEY_EDGE,
                    role = Role.Switch,
                    contentDescription = label,
                    style = keyStyle,
                    contentPadding = keyPadding,
                    modifier = keyModifier.weight(1f),
                )
            }
        }

        // Navigation & special keys row: 6 equal columns across the full width
        val keys = listOf(
            Triple("escape", stringResource(R.string.remote_key_esc), stringResource(R.string.remote_key_esc_desc)),
            Triple("tab", stringResource(R.string.remote_key_tab), stringResource(R.string.remote_key_tab)),
            Triple("left", "←", stringResource(R.string.remote_key_left_desc)),
            Triple("up", "↑", stringResource(R.string.remote_key_up_desc)),
            Triple("down", "↓", stringResource(R.string.remote_key_down_desc)),
            Triple("right", "→", stringResource(R.string.remote_key_right_desc)),
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for ((key, label, a11yLabel) in keys) {
                G7Keycap(
                    label = label,
                    onClick = { onKey(key, null) },
                    corner = SPECIAL_KEY_CORNER,
                    edge = SPECIAL_KEY_EDGE,
                    contentDescription = a11yLabel,
                    style = keyStyle,
                    contentPadding = keyPadding,
                    modifier = keyModifier.weight(1f),
                )
            }
        }

        if (showExtraKeys) {
            val extra = listOf(
                Triple("home", stringResource(R.string.remote_key_home), stringResource(R.string.remote_key_home)),
                Triple("end", stringResource(R.string.remote_key_end), stringResource(R.string.remote_key_end)),
                Triple("delete", stringResource(R.string.remote_key_del), stringResource(R.string.remote_key_del_desc)),
                Triple("copy", stringResource(R.string.remote_key_copy), stringResource(R.string.remote_key_copy)),
                Triple("paste", stringResource(R.string.remote_key_paste), stringResource(R.string.remote_key_paste)),
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                for ((key, label, a11yLabel) in extra) {
                    val isShortcut = key == "copy" || key == "paste"
                    G7Keycap(
                        label = label,
                        onClick = { onKey(key, if (isShortcut) emptyList() else null) },
                        // Something new was copied on the PC: the key that pastes it stands out.
                        ready = key == "paste" && pasteReady,
                        corner = SPECIAL_KEY_CORNER,
                        edge = SPECIAL_KEY_EDGE,
                        contentDescription = a11yLabel,
                        style = keyStyle,
                        contentPadding = keyPadding,
                        modifier = keyModifier.weight(1f),
                    )
                }
            }
        }
    }
}

/** A special key's corners and the edge under its face. */
private val SPECIAL_KEY_CORNER = 11.dp
private val SPECIAL_KEY_EDGE = 4.dp

/** The edge under the round send and dictate key. */
private val SEND_KEY_EDGE = 4.dp
