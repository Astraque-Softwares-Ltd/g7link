// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents.voice

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.view.accessibility.AccessibilityManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import kotlinx.coroutines.delay

/** SpeechRecognizer's language errors (Android 12): no offline model for the language. */
private const val ERROR_LANGUAGE_NOT_SUPPORTED = 12
private const val ERROR_LANGUAGE_UNAVAILABLE = 13

/** What the mic is doing now. */
sealed interface DictationState {
    data object Idle : DictationState

    /** Listening; [partial] is what's heard so far, [tap] when a tap (not a release) stops it. */
    data class Listening(val partial: String, val tap: Boolean) : DictationState

    /** The microphone isn't allowed: "Allow the microphone to speak a reply." */
    data object NoMic : DictationState

    /** The recogniser has no offline model for the phone's language. */
    data object NoPack : DictationState
}

/**
 * Speaking a reply (contract §V8.1): only Android's on-device recogniser,
 * with offline preferred; nothing else is ever asked. What's heard is handed
 * to [onHeard] once, when the hold ends; it's appended to a box, never sent.
 * Audio isn't kept, and the language is the system's.
 */
@Stable
class AgentDictation internal constructor(private val context: Context) {
    var state: DictationState by mutableStateOf(DictationState.Idle)
        private set

    /** Whether this phone can do it at all (Android 13 and an on-device recogniser). */
    val available: Boolean = AgentVoiceSettings.micAvailable(context)

    /** Listening under a held finger: the box says "Release to add it to your message". */
    val holding: Boolean get() = (state as? DictationState.Listening)?.tap == false

    internal var onHeard: (String) -> Unit = {}
    internal var askMic: () -> Unit = {}
    private var recognizer: SpeechRecognizer? = null
    private var lastPartial = ""

    /** A press (or a tap in tap mode): true when listening started. */
    fun press(tap: Boolean): Boolean {
        if (!available || state is DictationState.Listening) return false
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            // Asked at the first hold, with the reason over the box; it stays if the answer is no.
            state = DictationState.NoMic
            askMic()
            return false
        }
        return listen(tap)
    }

    /** The finger lifted (or the second tap): what was heard is added. */
    fun release() {
        val shown = recognizer ?: return
        runCatching { shown.stopListening() }
    }

    /** Slid away, or the screen went: nothing is added. */
    fun cancel() {
        drop()
        if (state is DictationState.Listening) state = DictationState.Idle
    }

    /** Tap to talk: the first tap starts, the next stops. */
    fun toggle(tap: Boolean) {
        if (state is DictationState.Listening) release() else press(tap)
    }

    internal fun micAnswered(granted: Boolean) {
        state = if (granted) DictationState.Idle else DictationState.NoMic
    }

    private fun listen(tap: Boolean): Boolean {
        drop()
        // The on-device recogniser only: never createSpeechRecognizer, which may send audio away.
        val created = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            runCatching { SpeechRecognizer.createOnDeviceSpeechRecognizer(context) }.getOrNull()
        } else {
            null
        }
        if (created == null) {
            state = DictationState.NoPack
            return false
        }
        lastPartial = ""
        created.setRecognitionListener(Listener(created))
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            .putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
        recognizer = created
        state = DictationState.Listening(partial = "", tap = tap)
        val started = runCatching { created.startListening(intent) }.isSuccess
        if (!started) {
            drop()
            state = DictationState.NoPack
        }
        return started
    }

    private fun drop() {
        val shown = recognizer ?: return
        recognizer = null
        runCatching {
            shown.cancel()
            shown.destroy()
        }
    }

    private fun end(heard: AgentVoice.Heard) {
        drop()
        state = when (heard) {
            is AgentVoice.Heard.Words -> {
                onHeard(heard.text)
                DictationState.Idle
            }
            AgentVoice.Heard.NoMic -> DictationState.NoMic
            AgentVoice.Heard.NoPack -> DictationState.NoPack
            AgentVoice.Heard.Nothing, AgentVoice.Heard.Failed -> DictationState.Idle
        }
    }

    private inner class Listener(private val owner: SpeechRecognizer) : RecognitionListener {
        private fun current(): Boolean = recognizer === owner

        override fun onReadyForSpeech(params: Bundle?) = Unit

        override fun onBeginningOfSpeech() = Unit

        override fun onRmsChanged(rmsdB: Float) = Unit

        override fun onBufferReceived(buffer: ByteArray?) = Unit

        override fun onEndOfSpeech() = Unit

        override fun onEvent(eventType: Int, params: Bundle?) = Unit

        override fun onPartialResults(partialResults: Bundle?) {
            if (!current()) return
            val partial = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.trim().orEmpty()
            if (partial.isEmpty()) return
            lastPartial = partial
            (state as? DictationState.Listening)?.let { state = it.copy(partial = partial) }
        }

        override fun onResults(results: Bundle?) {
            if (!current()) return
            end(AgentVoice.results(results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull(), lastPartial))
        }

        override fun onError(error: Int) {
            if (!current()) return
            val kind = when (error) {
                SpeechRecognizer.ERROR_NO_MATCH -> AgentVoice.RecogniserError.NO_MATCH
                SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> AgentVoice.RecogniserError.SPEECH_TIMEOUT
                SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> AgentVoice.RecogniserError.PERMISSIONS
                ERROR_LANGUAGE_NOT_SUPPORTED, ERROR_LANGUAGE_UNAVAILABLE -> AgentVoice.RecogniserError.LANGUAGE
                else -> AgentVoice.RecogniserError.OTHER
            }
            end(AgentVoice.error(kind, lastPartial))
        }
    }
}

/**
 * The mic of a reply box: [onHeard] gets what was said when a hold ends.
 * Listening stops after a minute, and when the box leaves the screen.
 */
@Composable
fun rememberAgentDictation(onHeard: (String) -> Unit): AgentDictation {
    val context = LocalContext.current
    val dictation = remember(context) { AgentDictation(context) }
    val heard by rememberUpdatedState(onHeard)
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted -> dictation.micAnswered(granted) }
    dictation.onHeard = { heard(it) }
    dictation.askMic = { runCatching { ask.launch(Manifest.permission.RECORD_AUDIO) } }
    DisposableEffect(dictation) { onDispose { dictation.cancel() } }
    val listening = dictation.state is DictationState.Listening
    LaunchedEffect(listening) {
        if (!listening) return@LaunchedEffect
        delay(AgentVoice.MAX_HOLD_MS)
        dictation.release()
    }
    return dictation
}

/** Tap to talk: chosen in settings, or TalkBack is on (a hold and a slide can't be done with it). */
@Composable
fun rememberTapToTalk(): Boolean {
    val context = LocalContext.current
    val exploring = remember(context) { context.getSystemService(AccessibilityManager::class.java)?.isTouchExplorationEnabled == true }
    return AgentVoice.tapToTalk(AgentVoiceSettings.tapToTalk(context), exploring)
}

/** How far past the button a finger slides before the hold is cancelled. */
private val SLIDE_CANCEL = 24.dp

/**
 * The mic button: hold to speak, slide away to cancel; with [tap], a tap
 * starts and a tap stops. Shown only where the phone can recognise speech on
 * the device (the caller checks [AgentDictation.available]).
 */
@Composable
fun AgentMicButton(dictation: AgentDictation, tap: Boolean, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val scheme = MaterialTheme.colorScheme
    val listening = dictation.state is DictationState.Listening
    val haptics = LocalHapticFeedback.current
    val slop = with(LocalDensity.current) { SLIDE_CANCEL.toPx() }
    val description = stringResource(if (tap) R.string.agents_voice_tap_to_talk else R.string.agents_voice_hold)
    val gesture = if (!enabled) {
        Modifier
    } else if (tap) {
        Modifier.pointerInput(dictation) {
            awaitEachGesture {
                awaitFirstDown().consume()
                val up = waitForUpOrCancellation()
                if (up != null) {
                    up.consume()
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    dictation.toggle(tap = true)
                }
            }
        }
    } else {
        Modifier.pointerInput(dictation) {
            awaitEachGesture {
                val down = awaitFirstDown()
                down.consume()
                if (!dictation.press(tap = false)) return@awaitEachGesture
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                var ended = false
                try {
                    var cancelled = false
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        if (!change.pressed) {
                            change.consume()
                            break
                        }
                        val at = change.position
                        if (at.x < -slop || at.y < -slop || at.x > size.width + slop || at.y > size.height + slop) {
                            cancelled = true
                            break
                        }
                        change.consume()
                    }
                    ended = true
                    if (cancelled) dictation.cancel() else dictation.release()
                } finally {
                    if (!ended) dictation.cancel()
                }
            }
        }
    }
    Box(
        modifier = modifier
            .size(44.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (listening) scheme.surfaceContainerHigh else Color.Transparent)
            .then(gesture)
            .semantics {
                contentDescription = description
                role = Role.Button
                if (enabled) {
                    onClick {
                        dictation.toggle(tap = true)
                        true
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            G7Icons.mic,
            contentDescription = null,
            tint = when {
                !enabled -> scheme.onSurfaceVariant
                listening -> scheme.primary
                else -> scheme.onSurface
            },
            modifier = Modifier.size(20.dp),
        )
    }
}

/**
 * Above the reply box while listening (board 05b): "Listening…", "Slide away
 * to cancel" for a hold, and the words as they're heard. When it can't listen,
 * why, in one line.
 */
@Composable
fun AgentVoiceStrip(dictation: AgentDictation, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    when (val state = dictation.state) {
        is DictationState.Listening -> Column(
            modifier = modifier
                .fillMaxWidth()
                .padding(bottom = 6.dp)
                .clip(RoundedCornerShape(10.dp))
                .border(1.dp, scheme.primary, RoundedCornerShape(10.dp))
                .background(scheme.surfaceContainer)
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(G7Icons.mic, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(16.dp))
                Text(
                    stringResource(R.string.agents_voice_listening),
                    style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.SemiBold),
                    color = scheme.primary,
                )
                Spacer(Modifier.weight(1f))
                if (!state.tap) {
                    Text(stringResource(R.string.agents_voice_slide_cancel), style = MaterialTheme.typography.labelMedium, color = scheme.onSurfaceVariant)
                }
            }
            if (state.partial.isNotEmpty()) Text(state.partial, style = MaterialTheme.typography.bodyLarge, maxLines = 4)
        }
        DictationState.NoMic -> VoiceLine(G7Icons.micOff, stringResource(R.string.agents_voice_allow_mic), modifier)
        DictationState.NoPack -> VoiceLine(G7Icons.micOff, stringResource(R.string.agents_voice_needs_pack), modifier)
        DictationState.Idle -> Unit
    }
}

@Composable
private fun VoiceLine(icon: androidx.compose.ui.graphics.vector.ImageVector, text: String, modifier: Modifier) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(bottom = 6.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(icon, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        Text(text, style = MaterialTheme.typography.bodySmall)
    }
}
