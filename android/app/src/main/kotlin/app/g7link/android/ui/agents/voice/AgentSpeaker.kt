// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents.voice

import android.app.KeyguardManager
import android.app.NotificationManager
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import android.speech.tts.Voice
import app.g7link.android.R
import app.g7link.android.notifications.AgentNotice
import app.g7link.android.notifications.AgentNotifications
import app.g7link.android.notifications.FinishKind
import kotlin.coroutines.resume
import kotlinx.coroutines.suspendCancellableCoroutine

/**
 * Read it to me (contract §V8.2): one sentence when a thread ends, through
 * headphones (the speaker only when allowed and unlocked), with a voice that
 * needs no network, and only while neither the phone nor the PC is in Do Not
 * Disturb and no call is on. Nothing is recorded or sent; the sentence is made
 * on the phone from what its notification already shows. Everything runs on
 * the main thread; the engine is let go a minute after the last sentence.
 */
object AgentSpeaker {
    private const val IDLE_RELEASE_MS = 60_000L

    private val main by lazy { Handler(Looper.getMainLooper()) }
    private val queue = SpeechQueue()
    private var engine: TextToSpeech? = null

    /** Null while the engine starts; then whether it has an offline voice in the app's language. */
    private var ready: Boolean? = null
    private var voice: Voice? = null
    private var focus: AudioFocusRequest? = null
    private var utteranceId: String? = null
    private var counter = 0L
    private var appContext: Context? = null
    private val release = Runnable { releaseEngine() }

    private val attributes: AudioAttributes by lazy {
        AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_ASSISTANT)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
    }

    /**
     * A thread's end was posted (not held while open on the PC): says it when
     * "Read results aloud" is on for its PC and everything else allows.
     * [pcDnd]: the PC said it's in Do Not Disturb and that's still fresh.
     */
    fun finished(context: Context, notice: AgentNotice.Finished, pcDnd: Boolean) {
        val app = context.applicationContext
        // Off (the default): the engine isn't even started.
        if (!AgentVoiceSettings.readAloud(app, notice.pcId)) return
        val line = AgentVoice.line(notice.kind, notice.agent, notice.text, notice.folder, notice.title, ::speakable) ?: return
        val check = AgentVoice.SpeakCheck(
            enabled = true,
            speaker = AgentVoiceSettings.speaker(app, notice.pcId),
            held = false,
            phoneDnd = phoneDnd(app),
            pcDnd = pcDnd,
            inCall = inCall(app),
            route = route(app),
            locked = locked(app),
            // Checked once the engine is up; nothing is said without one.
            voice = true,
        )
        if (!AgentVoice.maySpeak(check)) return
        val text = when (line.kind) {
            FinishKind.DONE -> app.getString(R.string.agents_speak_finished, line.agent, line.detail)
            FinishKind.FAILED -> app.getString(R.string.agents_speak_failed, line.agent, line.detail)
            FinishKind.PLAN -> app.getString(R.string.agents_speak_plan, line.agent, line.detail)
            FinishKind.PAUSED -> return
        }
        val utterance = SpeechQueue.Utterance("${notice.pcId}/${notice.threadId}", notice.pcId, notice.threadId, text)
        main.post { enqueue(app, utterance) }
    }

    /** Stop speaking: what's said stops and nothing waits (the notification's button, or the setting turned off). */
    fun stop(context: Context) {
        val app = context.applicationContext
        main.post {
            val said = queue.clear()
            utteranceId = null
            runCatching { engine?.stop() }
            abandonFocus(app)
            said.distinctBy { it.key }.forEach { AgentNotifications.speaking(app, it.pcId, it.threadId, on = false) }
            main.removeCallbacks(release)
            main.postDelayed(release, IDLE_RELEASE_MS)
        }
    }

    /** Whether an offline voice in the app's language is installed (the setting is off without one). */
    suspend fun hasOfflineVoice(context: Context): Boolean = suspendCancellableCoroutine { cont ->
        val app = context.applicationContext
        main.post {
            val known = ready
            if (engine != null && known != null) {
                if (cont.isActive) cont.resume(known)
                return@post
            }
            var probe: TextToSpeech? = null
            probe = runCatching {
                TextToSpeech(app) { status ->
                    val found = status == TextToSpeech.SUCCESS && probe?.let { pick(app, it) } != null
                    runCatching { probe?.shutdown() }
                    if (cont.isActive) cont.resume(found)
                }
            }.getOrNull()
            when {
                probe == null -> if (cont.isActive) cont.resume(false)
                // It failed inside its constructor, before it was known here.
                !cont.isActive -> runCatching { probe.shutdown() }
            }
        }
    }

    private fun enqueue(app: Context, utterance: SpeechQueue.Utterance) {
        appContext = app
        main.removeCallbacks(release)
        if (queue.offer(utterance)) start(app)
    }

    /** Says what the queue holds now, starting the engine first when it isn't. */
    private fun start(app: Context) {
        val current = queue.speaking ?: return
        val tts = engine
        when {
            tts == null -> {
                ready = null
                engine = runCatching { TextToSpeech(app) { status -> main.post { started(app, status) } } }.getOrNull()
                if (engine == null) failAll(app)
            }
            ready == null -> Unit
            ready == false -> failAll(app)
            else -> say(app, tts, current)
        }
    }

    private fun started(app: Context, status: Int) {
        val tts = engine ?: return
        voice = if (status == TextToSpeech.SUCCESS) pick(app, tts) else null
        ready = voice != null
        if (ready == true) {
            tts.setOnUtteranceProgressListener(Progress)
            queue.speaking?.let { say(app, tts, it) }
        } else {
            failAll(app)
        }
    }

    private fun say(app: Context, tts: TextToSpeech, utterance: SpeechQueue.Utterance) {
        // Said only while the sound still goes where it may: a queued one is checked again now.
        val allowed = AgentVoice.maySpeak(
            AgentVoice.SpeakCheck(
                enabled = AgentVoiceSettings.readAloud(app, utterance.pcId),
                speaker = AgentVoiceSettings.speaker(app, utterance.pcId),
                held = false,
                phoneDnd = phoneDnd(app),
                pcDnd = false,
                inCall = inCall(app),
                route = route(app),
                locked = locked(app),
                voice = voice != null,
            ),
        )
        if (!allowed || !requestFocus(app)) {
            next(app)
            return
        }
        val id = "agent-speak-${++counter}"
        utteranceId = id
        runCatching {
            voice?.let { tts.setVoice(it) }
            tts.setAudioAttributes(attributes)
            tts.speak(utterance.text, TextToSpeech.QUEUE_FLUSH, Bundle(), id)
        }.onFailure {
            utteranceId = null
            next(app)
            return
        }
        AgentNotifications.speaking(app, utterance.pcId, utterance.threadId, on = true)
    }

    /** The sentence [id] ended, was stopped, or failed. */
    private fun ended(id: String) {
        val app = appContext ?: return
        if (id != utteranceId) return
        utteranceId = null
        queue.speaking?.let { AgentNotifications.speaking(app, it.pcId, it.threadId, on = false) }
        next(app)
    }

    private fun next(app: Context) {
        val upcoming = queue.next()
        if (upcoming != null) {
            start(app)
        } else {
            abandonFocus(app)
            main.removeCallbacks(release)
            main.postDelayed(release, IDLE_RELEASE_MS)
        }
    }

    /** No engine or no offline voice: nothing is said, and nothing waits. */
    private fun failAll(app: Context) {
        queue.clear()
        utteranceId = null
        abandonFocus(app)
        main.removeCallbacks(release)
        main.postDelayed(release, IDLE_RELEASE_MS)
    }

    private fun releaseEngine() {
        if (queue.speaking != null) return
        runCatching { engine?.shutdown() }
        engine = null
        ready = null
        voice = null
    }

    private object Progress : UtteranceProgressListener() {
        override fun onStart(utteranceId: String?) = Unit

        override fun onDone(utteranceId: String?) {
            utteranceId?.let { main.post { ended(it) } }
        }

        @Deprecated("Deprecated in Java")
        override fun onError(utteranceId: String?) {
            utteranceId?.let { main.post { ended(it) } }
        }

        override fun onError(utteranceId: String?, errorCode: Int) {
            utteranceId?.let { main.post { ended(it) } }
        }

        override fun onStop(utteranceId: String?, interrupted: Boolean) {
            utteranceId?.let { main.post { ended(it) } }
        }
    }

    /** An installed voice that needs no network, in the language the app speaks. */
    private fun pick(app: Context, tts: TextToSpeech): Voice? {
        val locale = app.resources.configuration.locales.get(0) ?: return null
        val voices = runCatching { tts.voices }.getOrNull().orEmpty().filterNotNull()
        val infos = voices.map { voice ->
            AgentVoice.VoiceInfo(
                name = voice.name,
                language = voice.locale?.language.orEmpty(),
                country = voice.locale?.country.orEmpty(),
                needsNetwork = voice.isNetworkConnectionRequired,
                installed = TextToSpeech.Engine.KEY_FEATURE_NOT_INSTALLED !in voice.features.orEmpty(),
            )
        }
        val chosen = AgentVoice.offlineVoice(infos, locale.language, locale.country) ?: return null
        return voices.firstOrNull { it.name == chosen.name }
    }

    /** Markdown to plain words, cut at a sentence end (the render crate's `speakable`). */
    private fun speakable(text: String, maxChars: Int): String =
        runCatching { app.g7link.core.agentSpeakable(text, maxChars.toUInt()) }.getOrNull()
            ?: text.lineSequence().map { it.trim() }.filter { it.isNotEmpty() && !it.startsWith("```") }.joinToString(" ").take(maxChars)

    private fun requestFocus(app: Context): Boolean {
        val audio = app.getSystemService(AudioManager::class.java) ?: return false
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
            .setAudioAttributes(attributes)
            .build()
        val granted = runCatching { audio.requestAudioFocus(request) }.getOrDefault(AudioManager.AUDIOFOCUS_REQUEST_FAILED)
        if (granted != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) return false
        focus?.let { previous -> runCatching { audio.abandonAudioFocusRequest(previous) } }
        focus = request
        return true
    }

    private fun abandonFocus(app: Context) {
        val held = focus ?: return
        focus = null
        runCatching { app.getSystemService(AudioManager::class.java)?.abandonAudioFocusRequest(held) }
    }

    private fun route(app: Context): AgentVoice.Route {
        val audio = app.getSystemService(AudioManager::class.java) ?: return AgentVoice.Route.SPEAKER
        val types = runCatching { audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).map { it.type } }.getOrDefault(emptyList())
        return AgentVoice.route(types)
    }

    /** A call rings, is under way or is being screened (any mode but the normal one). */
    private fun inCall(app: Context): Boolean =
        app.getSystemService(AudioManager::class.java)?.let { it.mode != AudioManager.MODE_NORMAL } ?: false

    private fun phoneDnd(app: Context): Boolean {
        val filter = app.getSystemService(NotificationManager::class.java)?.currentInterruptionFilter ?: return false
        return filter != NotificationManager.INTERRUPTION_FILTER_ALL && filter != NotificationManager.INTERRUPTION_FILTER_UNKNOWN
    }

    private fun locked(app: Context): Boolean = app.getSystemService(KeyguardManager::class.java)?.isDeviceLocked ?: true
}
