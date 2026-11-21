// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents.voice

import app.g7link.android.notifications.FinishKind
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Voice replies and spoken results (contract §V8.1, §V8.2). */
class AgentVoiceModelTest {
    @Test
    fun `the mic shows only with an on-device recogniser on Android 13 or later`() {
        assertTrue(AgentVoice.micShown(sdkInt = 33, onDeviceAvailable = true))
        assertTrue(AgentVoice.micShown(sdkInt = 36, onDeviceAvailable = true))
        assertFalse(AgentVoice.micShown(sdkInt = 36, onDeviceAvailable = false))
        // Android 12 has no on-device recogniser to ask for: no mic, never the default one.
        assertFalse(AgentVoice.micShown(sdkInt = 32, onDeviceAvailable = true))
        assertFalse(AgentVoice.micShown(sdkInt = 26, onDeviceAvailable = false))
    }

    @Test
    fun `tap to talk when chosen or with TalkBack, else hold`() {
        assertFalse(AgentVoice.tapToTalk(setting = false, touchExploration = false))
        assertTrue(AgentVoice.tapToTalk(setting = true, touchExploration = false))
        assertTrue(AgentVoice.tapToTalk(setting = false, touchExploration = true))
    }

    @Test
    fun `what was heard is appended to the box and selected, never sent`() {
        val empty = AgentVoice.appended("", "  Sasa ongeza backoff  ")!!
        assertEquals("Sasa ongeza backoff", empty.text)
        assertEquals(0, empty.start)
        assertEquals(empty.text.length, empty.end)

        val after = AgentVoice.appended("Look at queue.rs.", "Then add a backoff.")!!
        assertEquals("Look at queue.rs. Then add a backoff.", after.text)
        assertEquals("Then add a backoff.", after.text.substring(after.start, after.end))

        // A box ending in a space or a line break gets no second space; spoken line breaks become spaces.
        assertEquals("Hello world", AgentVoice.appended("Hello ", "world")!!.text)
        assertEquals("Hello\nworld again", AgentVoice.appended("Hello\n", "world\nagain")!!.text)

        // Nothing heard adds nothing.
        assertNull(AgentVoice.appended("Kept as is", "   "))
        assertNull(AgentVoice.appended("Kept as is", ""))
    }

    @Test
    fun `what doesn't fit is left out at a character boundary`() {
        val cut = AgentVoice.appended("ab", "c😀d", maxChars = 5)!!
        // "ab" + " " leaves two units: "c" and half of the emoji, which is dropped.
        assertEquals("ab c", cut.text)
        assertNull(AgentVoice.appended("abcde", "more", maxChars = 5))
    }

    @Test
    fun `a cancelled or empty hold adds nothing, a timeout keeps what was heard`() {
        assertEquals(AgentVoice.Heard.Words("weka backoff"), AgentVoice.results("weka backoff", ""))
        assertEquals(AgentVoice.Heard.Words("partial words"), AgentVoice.results(null, "partial words"))
        assertEquals(AgentVoice.Heard.Words("partial words"), AgentVoice.results("  ", "partial words"))
        assertEquals(AgentVoice.Heard.Nothing, AgentVoice.results(null, ""))

        assertEquals(AgentVoice.Heard.Words("so far"), AgentVoice.error(AgentVoice.RecogniserError.SPEECH_TIMEOUT, "so far"))
        assertEquals(AgentVoice.Heard.Nothing, AgentVoice.error(AgentVoice.RecogniserError.NO_MATCH, ""))
        assertEquals(AgentVoice.Heard.NoMic, AgentVoice.error(AgentVoice.RecogniserError.PERMISSIONS, "ignored"))
        assertEquals(AgentVoice.Heard.NoPack, AgentVoice.error(AgentVoice.RecogniserError.LANGUAGE, ""))
        assertEquals(AgentVoice.Heard.Failed, AgentVoice.error(AgentVoice.RecogniserError.OTHER, "dropped"))
    }

    private val ok = AgentVoice.SpeakCheck(
        enabled = true,
        speaker = false,
        held = false,
        phoneDnd = false,
        pcDnd = false,
        inCall = false,
        route = AgentVoice.Route.HEADPHONES,
        locked = true,
        voice = true,
    )

    @Test
    fun `results are spoken through headphones, and the speaker only when allowed and unlocked`() {
        assertTrue(AgentVoice.maySpeak(ok))
        val speaker = ok.copy(route = AgentVoice.Route.SPEAKER)
        assertFalse(AgentVoice.maySpeak(speaker))
        assertFalse(AgentVoice.maySpeak(speaker.copy(speaker = true, locked = true)))
        assertTrue(AgentVoice.maySpeak(speaker.copy(speaker = true, locked = false)))
    }

    @Test
    fun `nothing is spoken when off, in Do Not Disturb, in a call, held, or without an offline voice`() {
        assertFalse(AgentVoice.maySpeak(ok.copy(enabled = false)))
        assertFalse(AgentVoice.maySpeak(ok.copy(phoneDnd = true)))
        assertFalse(AgentVoice.maySpeak(ok.copy(pcDnd = true)))
        assertFalse(AgentVoice.maySpeak(ok.copy(inCall = true)))
        assertFalse(AgentVoice.maySpeak(ok.copy(held = true)))
        assertFalse(AgentVoice.maySpeak(ok.copy(voice = false)))
    }

    @Test
    fun `headphones are wired, USB, Bluetooth A2DP or LE Audio`() {
        // AudioDeviceInfo: 2 built-in speaker, 3 wired headset, 4 wired headphones, 8 A2DP, 22 USB headset, 26 BLE headset, 27 BLE speaker.
        assertEquals(AgentVoice.Route.SPEAKER, AgentVoice.route(listOf(2)))
        assertEquals(AgentVoice.Route.SPEAKER, AgentVoice.route(emptyList()))
        assertEquals(AgentVoice.Route.SPEAKER, AgentVoice.route(listOf(2, 27)))
        for (type in listOf(3, 4, 8, 22, 26)) assertEquals(AgentVoice.Route.HEADPHONES, AgentVoice.route(listOf(2, type)))
    }

    @Test
    fun `a voice that needs the network is never used, nor one in another language`() {
        val online = AgentVoice.VoiceInfo("en-us-x-net", "en", "US", needsNetwork = true, installed = true)
        val missing = AgentVoice.VoiceInfo("en-us-x-local-missing", "en", "US", needsNetwork = false, installed = false)
        val german = AgentVoice.VoiceInfo("de-de-x-local", "de", "DE", needsNetwork = false, installed = true)
        assertNull(AgentVoice.offlineVoice(listOf(online, missing, german), "en", "US"))

        val british = AgentVoice.VoiceInfo("en-gb-x-local", "en", "GB", needsNetwork = false, installed = true)
        val american = AgentVoice.VoiceInfo("en-us-x-local", "en", "US", needsNetwork = false, installed = true)
        assertEquals(american, AgentVoice.offlineVoice(listOf(online, british, american), "en", "US"))
        // No voice for the country: one in the language.
        assertEquals(british, AgentVoice.offlineVoice(listOf(online, british), "en", "KE"))
    }

    private val speakable: (String, Int) -> String = { text, max -> text.replace("`", "").take(max) }

    @Test
    fun `one sentence says who finished and how, a paused thread says nothing`() {
        assertEquals(
            AgentVoice.Line(FinishKind.DONE, "Claude", "A row now leaves the queue only after the server acknowledges it."),
            AgentVoice.line(FinishKind.DONE, "Claude", "A row now leaves the queue only after the server acknowledges it.", "duka-pos", "Stock sync", speakable),
        )
        assertEquals("cargo failed", AgentVoice.line(FinishKind.FAILED, "Codex", "`cargo` failed", null, "Fix", speakable)!!.detail)
        assertEquals("duka-pos", AgentVoice.line(FinishKind.PLAN, "Claude", "a long plan", "duka-pos", "Plan it", speakable)!!.detail)
        // Nothing to say but the title: the title.
        assertEquals("Stock sync", AgentVoice.line(FinishKind.DONE, "Claude", null, "duka-pos", "Stock sync", speakable)!!.detail)
        assertNull(AgentVoice.line(FinishKind.PAUSED, "Claude", "limit", "duka-pos", "Stock sync", speakable))
        assertNull(AgentVoice.line(FinishKind.DONE, "Claude", " ", null, " ", speakable))
        // The summary is cut to what's spoken.
        assertEquals(AgentVoice.SPEAK_MAX_CHARS, AgentVoice.line(FinishKind.DONE, "Claude", "x".repeat(500), null, "t", speakable)!!.detail.length)
    }

    @Test
    fun `one waits per thread, and a newer one for the same thread replaces it`() {
        val queue = SpeechQueue(max = 2)
        fun line(thread: String, text: String) = SpeechQueue.Utterance("pc/$thread", "pc", thread, text)
        assertTrue(queue.offer(line("a", "first")))
        assertFalse(queue.offer(line("b", "b one")))
        assertFalse(queue.offer(line("b", "b two")))
        assertEquals(1, queue.waitingCount)
        // The one being said isn't replaced: a newer one for it waits.
        assertFalse(queue.offer(line("a", "a again")))
        assertEquals(2, queue.waitingCount)
        assertFalse(queue.offer(line("c", "c one")))
        assertEquals(2, queue.waitingCount)
        assertEquals("a again", queue.next()!!.text)
        assertEquals("c one", queue.next()!!.text)
        assertNull(queue.next())
        assertTrue(queue.offer(line("d", "d")))
        assertEquals(1, queue.clear().size)
        assertNull(queue.speaking)
    }

    @Test
    fun `nothing under ui agents asks for the default recogniser`() {
        val root = listOf(File("src/main/kotlin"), File("app/src/main/kotlin")).first { it.isDirectory }
        val agents = File(root, "app/g7link/android/ui/agents")
        assertTrue(agents.isDirectory)
        val offenders = agents.walkTopDown().filter { it.isFile && it.extension == "kt" }.filter { file ->
            file.readText().contains("createSpeechRecognizer(")
        }.map { it.name }.toList()
        assertEquals(emptyList<String>(), offenders)
        // And the dictation asks for the on-device one, with offline preferred.
        val dictation = File(agents, "voice/AgentDictation.kt").readText()
        assertTrue(dictation.contains("createOnDeviceSpeechRecognizer("))
        assertTrue(dictation.contains("EXTRA_PREFER_OFFLINE"))
    }
}
