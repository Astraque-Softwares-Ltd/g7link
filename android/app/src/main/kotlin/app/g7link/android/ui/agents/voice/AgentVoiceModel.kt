// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents.voice

import app.g7link.android.notifications.FinishKind

/**
 * Voice replies and spoken results (contract §V8.1, §V8.2), as rules without
 * Android: when the mic shows, what a hold adds to the reply box, when a
 * finished thread may be said aloud and through what, which voice may say it,
 * and the one sentence it says. Speech stays on the phone: only an on-device
 * recogniser and a voice that needs no network are ever used.
 */
object AgentVoice {
    /** Android's on-device recogniser (`createOnDeviceSpeechRecognizer`) arrived in 13. */
    const val ON_DEVICE_SDK = 33

    /** The longest one hold listens. */
    const val MAX_HOLD_MS = 60_000L

    /** How much of a summary is spoken (`agent_speakable`'s cut, at a sentence end). */
    const val SPEAK_MAX_CHARS = 200

    /**
     * The mic shows only where speech can stay on the phone: Android 13 or
     * later with an on-device recogniser installed. There's no fallback to the
     * default recogniser, which may send audio away.
     */
    fun micShown(sdkInt: Int, onDeviceAvailable: Boolean): Boolean = sdkInt >= ON_DEVICE_SDK && onDeviceAvailable

    /** A tap starts and a tap stops when chosen, or with TalkBack on (it can't hold and slide). */
    fun tapToTalk(setting: Boolean, touchExploration: Boolean): Boolean = setting || touchExploration

    /** The reply box after a hold: its text, and the words just added, selected. */
    data class Appended(val text: String, val start: Int, val end: Int)

    /**
     * [heard] added to the end of what the box holds, after a space when the
     * box doesn't end with one, and selected; nothing else changes and nothing
     * is sent. Null when nothing was heard. What doesn't fit [maxChars] is left
     * out at a character boundary.
     */
    fun appended(box: String, heard: String, maxChars: Int = Int.MAX_VALUE): Appended? {
        val words = heard.trim().replace(LINE_BREAKS, " ")
        if (words.isEmpty()) return null
        val gap = if (box.isEmpty() || box.last().isWhitespace()) "" else " "
        val room = maxChars - box.length - gap.length
        if (room <= 0) return null
        var cut = words.take(room)
        // Never half a surrogate pair.
        if (cut.isNotEmpty() && cut.length < words.length && Character.isHighSurrogate(cut.last())) cut = cut.dropLast(1)
        if (cut.isBlank()) return null
        val start = box.length + gap.length
        return Appended(box + gap + cut, start, start + cut.length)
    }

    /** How a hold ended. */
    sealed interface Heard {
        /** Words to add to the box. */
        data class Words(val text: String) : Heard

        /** Nothing was said, or it was cancelled: the box stays as it was. */
        data object Nothing : Heard

        /** The microphone isn't allowed. */
        data object NoMic : Heard

        /** The recogniser has no offline model for the phone's language. */
        data object NoPack : Heard

        /** Something else went wrong; nothing is added. */
        data object Failed : Heard
    }

    /** What the recogniser said went wrong, in the rules' words. */
    enum class RecogniserError { NO_MATCH, SPEECH_TIMEOUT, PERMISSIONS, LANGUAGE, OTHER }

    /** The recogniser's results: its first, else the last partial; empty is nothing. */
    fun results(first: String?, lastPartial: String): Heard {
        val words = first?.takeIf { it.isNotBlank() } ?: lastPartial
        return if (words.isBlank()) Heard.Nothing else Heard.Words(words.trim())
    }

    /**
     * An error: a hold that heard something before it timed out keeps what it
     * heard; a missing language pack and a refused microphone say so.
     */
    fun error(error: RecogniserError, lastPartial: String): Heard = when (error) {
        RecogniserError.NO_MATCH, RecogniserError.SPEECH_TIMEOUT ->
            if (lastPartial.isNotBlank()) Heard.Words(lastPartial.trim()) else Heard.Nothing
        RecogniserError.PERMISSIONS -> Heard.NoMic
        RecogniserError.LANGUAGE -> Heard.NoPack
        RecogniserError.OTHER -> Heard.Failed
    }

    // ---- Read it to me (§V8.2) ----

    /** Where the phone's sound would go now. */
    enum class Route { HEADPHONES, SPEAKER }

    /** AudioDeviceInfo types (API 26 and later) that are in or on the ears: wired, USB-C, Bluetooth A2DP, LE Audio. */
    private const val TYPE_WIRED_HEADSET = 3
    private const val TYPE_WIRED_HEADPHONES = 4
    private const val TYPE_BLUETOOTH_A2DP = 8
    private const val TYPE_USB_HEADSET = 22
    private const val TYPE_BLE_HEADSET = 26
    private val PRIVATE_OUTPUTS = setOf(TYPE_WIRED_HEADSET, TYPE_WIRED_HEADPHONES, TYPE_BLUETOOTH_A2DP, TYPE_USB_HEADSET, TYPE_BLE_HEADSET)

    /** Headphones or earbuds among the connected outputs (they take the sound); else the speaker. */
    fun route(outputTypes: Collection<Int>): Route = if (outputTypes.any { it in PRIVATE_OUTPUTS }) Route.HEADPHONES else Route.SPEAKER

    /** Everything that decides whether a finished thread is said aloud. */
    data class SpeakCheck(
        /** "Read results aloud", for this PC (off until turned on). */
        val enabled: Boolean,
        /** "Also on the phone's speaker" (off). */
        val speaker: Boolean,
        /** Its notification was held while it was open on the PC, or wasn't posted. */
        val held: Boolean,
        val phoneDnd: Boolean,
        val pcDnd: Boolean,
        /** A call is ringing or under way. */
        val inCall: Boolean,
        val route: Route,
        val locked: Boolean,
        /** An offline voice in the phone's language is installed. */
        val voice: Boolean,
    )

    /**
     * Said aloud only when turned on, with an offline voice, for a notice that
     * was posted, with neither the phone nor the PC in Do Not Disturb, no call,
     * and through headphones; the speaker only when that's allowed too and the
     * phone is unlocked.
     */
    fun maySpeak(check: SpeakCheck): Boolean =
        check.enabled && check.voice && !check.held && !check.phoneDnd && !check.pcDnd && !check.inCall &&
            (check.route == Route.HEADPHONES || (check.speaker && !check.locked))

    /** A text-to-speech voice as the rules see it. */
    data class VoiceInfo(
        val name: String,
        /** ISO 639 language, as `Locale.getLanguage` gives it. */
        val language: String,
        val country: String,
        val needsNetwork: Boolean,
        val installed: Boolean,
    )

    /**
     * The voice to speak with: one that needs no network and is installed, in
     * the phone's language, its country first. None in that language: nothing
     * is spoken (another language's voice would mangle the words).
     */
    fun offlineVoice(voices: List<VoiceInfo>, language: String, country: String): VoiceInfo? {
        val usable = voices.filter { !it.needsNetwork && it.installed && it.language.equals(language, ignoreCase = true) }
        return usable.firstOrNull { it.country.equals(country, ignoreCase = true) } ?: usable.minByOrNull { it.name }
    }

    /** What a finished thread's sentence says: whose and how it ended, and the words after the colon. */
    data class Line(val kind: FinishKind, val agent: String, val detail: String)

    /**
     * The one sentence for a thread that ended: done with its summary, failed
     * with the CLI's message, or a plan with the folder it's in. [speakable]
     * turns Markdown into plain words cut at a sentence end (the FFI's
     * `agent_speakable`). The title stands in when there's nothing else; a
     * thread paused at its limit isn't spoken.
     */
    fun line(
        kind: FinishKind,
        agent: String,
        text: String?,
        folder: String?,
        title: String,
        speakable: (String, Int) -> String,
    ): Line? {
        val words = { value: String? -> value?.takeIf { it.isNotBlank() }?.let { speakable(it, SPEAK_MAX_CHARS) }?.trim().orEmpty() }
        val detail = when (kind) {
            FinishKind.DONE, FinishKind.FAILED -> words(text).ifEmpty { words(title) }
            FinishKind.PLAN -> folder?.trim().orEmpty().ifEmpty { words(title) }
            FinishKind.PAUSED -> return null
        }
        if (detail.isEmpty() || agent.isBlank()) return null
        return Line(kind, agent, detail)
    }

    private val LINE_BREAKS = Regex("\\s*[\\r\\n]+\\s*")
}

/**
 * What is said and what waits (contract §V8.2): one sentence at a time, and
 * at most one waiting per thread; a newer one for the same thread replaces
 * the one waiting. [max] bounds the whole queue; the oldest waiting goes
 * first. Not thread-safe: the speaker holds it under its own lock.
 */
class SpeechQueue(private val max: Int = MAX_WAITING) {
    /** One sentence for [key] (a PC and a thread). */
    data class Utterance(val key: String, val pcId: String, val threadId: String, val text: String)

    var speaking: Utterance? = null
        private set
    private val waiting = ArrayDeque<Utterance>()

    /** Queues [utterance]; true when it should start now (nothing is being said). */
    fun offer(utterance: Utterance): Boolean {
        if (speaking == null) {
            speaking = utterance
            return true
        }
        val at = waiting.indexOfFirst { it.key == utterance.key }
        if (at >= 0) {
            waiting[at] = utterance
        } else {
            if (waiting.size >= max) waiting.removeFirst()
            waiting.addLast(utterance)
        }
        return false
    }

    /** The one being said ended: the next to say, if any. */
    fun next(): Utterance? {
        speaking = waiting.removeFirstOrNull()
        return speaking
    }

    /** Stop speaking: nothing is said and nothing waits. Returns what was said and waiting. */
    fun clear(): List<Utterance> {
        val all = listOfNotNull(speaking) + waiting
        speaking = null
        waiting.clear()
        return all
    }

    val waitingCount: Int get() = waiting.size

    companion object {
        const val MAX_WAITING = 4
    }
}
