// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The phone's phase 2 rules (contract §Q6.1, §Q6.3, §Q10 "P"; reconcile file). */
class AgentControlTest {
    // ---- Questions ----

    private fun ask(
        id: String,
        multi: Boolean = false,
        other: Boolean = true,
        options: List<String> = listOf("cargo test", "cargo nextest"),
    ) = QuestionAsk(id = id, header = "Test runner", text = "Which setup?", multi = multi, other = other, options = options.map { QuestionOption(it) })

    private fun question(vararg asks: QuestionAsk, secret: Boolean = false, token: String = "q1") = QuestionInfo(
        token = token,
        threadId = "t1",
        agent = "claude",
        itemId = "i9",
        secret = secret,
        asks = asks.toList(),
        createdMs = 1_000L,
        expiresMs = 7_201_000L,
    )

    @Test
    fun `a single-choice ask takes one option, and another replaces it`() {
        val q = question(ask("0"))
        var a = QuestionDock.answersFor(q, null)
        assertFalse(QuestionDock.canSend(q, a, held = false))
        a = QuestionDock.choose(q, a, "cargo test")
        a = QuestionDock.choose(q, a, "cargo nextest")
        assertEquals(setOf("cargo nextest"), a.picks["0"])
        assertTrue(QuestionDock.canSend(q, a, held = false))
        assertEquals(listOf(QuestionAnswer("0", listOf("cargo nextest"), null)), QuestionDock.answersOut(q, a))
        // A label the ask doesn't offer is never picked.
        assertEquals(a, QuestionDock.choose(q, a, "rm -rf"))
    }

    @Test
    fun `typing in the reply box picks Something else, and another option leaves the words unused`() {
        val q = question(ask("0"))
        var a = QuestionDock.answersFor(q, null)
        assertEquals(QuestionField.OTHER, QuestionDock.field(q, a))
        a = QuestionDock.typeOther(q, a, "Both, on CI")
        assertTrue("0" in a.otherOn)
        assertEquals("Both, on CI", QuestionDock.otherText(q, a))
        assertEquals(listOf(QuestionAnswer("0", emptyList(), "Both, on CI")), QuestionDock.answersOut(q, a))

        a = QuestionDock.choose(q, a, "cargo test")
        // The words stay in the box, but the answer is the option.
        assertEquals("Both, on CI", QuestionDock.otherText(q, a))
        assertEquals(listOf(QuestionAnswer("0", listOf("cargo test"), null)), QuestionDock.answersOut(q, a))
    }

    @Test
    fun `Send answers waits while Something else is picked with nothing typed`() {
        val q = question(ask("0"))
        var a = QuestionDock.chooseOther(q, QuestionDock.answersFor(q, null))
        assertFalse(QuestionDock.canSend(q, a, held = false))
        assertNull(QuestionDock.answersOut(q, a))
        a = QuestionDock.typeOther(q, a, "   ")
        assertFalse(QuestionDock.canSend(q, a, held = false))
        a = QuestionDock.typeOther(q, a, "x".repeat(QuestionDock.MAX_OTHER_BYTES + 1))
        assertFalse("past the wire's 500 bytes", QuestionDock.canSend(q, a, held = false))
        a = QuestionDock.typeOther(q, a, "Use the fake radio")
        assertTrue(QuestionDock.canSend(q, a, held = false))
        // Held by the typing guard: nothing goes until the pause.
        assertFalse(QuestionDock.canSend(q, a, held = true))
    }

    @Test
    fun `a multi-choice ask checks several, with its own words as well`() {
        val q = question(ask("0", multi = true, options = listOf("a", "b", "c")))
        var a = QuestionDock.answersFor(q, null)
        a = QuestionDock.choose(q, a, "c")
        a = QuestionDock.choose(q, a, "a")
        a = QuestionDock.choose(q, a, "c")
        assertEquals(setOf("a"), a.picks["0"])
        a = QuestionDock.typeOther(q, a, "and d")
        assertEquals(listOf(QuestionAnswer("0", listOf("a"), "and d")), QuestionDock.answersOut(q, a))
    }

    @Test
    fun `several asks go one at a time, Next on an answered one, Back, Send answers on the last`() {
        val q = question(ask("0"), ask("1", other = false), ask("2"))
        var a = QuestionDock.answersFor(q, null)
        assertFalse(QuestionDock.canNext(q, a, held = false))
        a = QuestionDock.next(q, a)
        assertEquals("Next does nothing on an unanswered ask", 0, a.index)
        a = QuestionDock.next(q, QuestionDock.choose(q, a, "cargo test"))
        assertEquals(1, a.index)
        // An ask without its own answer: the reply box is closed for it.
        assertEquals(QuestionField.CHOOSE_ABOVE, QuestionDock.field(q, a))
        assertEquals(a, QuestionDock.typeOther(q, a, "ignored"))
        a = QuestionDock.next(q, QuestionDock.choose(q, a, "cargo nextest"))
        assertTrue(QuestionDock.isLast(q, a))
        assertFalse(QuestionDock.canNext(q, a, held = false))
        a = QuestionDock.typeOther(q, a, "Mine")
        assertTrue(QuestionDock.canSend(q, a, held = false))
        val back = QuestionDock.back(a)
        assertEquals(1, back.index)
        assertEquals(
            listOf(
                QuestionAnswer("0", listOf("cargo test"), null),
                QuestionAnswer("1", listOf("cargo nextest"), null),
                QuestionAnswer("2", emptyList(), "Mine"),
            ),
            QuestionDock.answersOut(q, a),
        )
    }

    @Test
    fun `a secret question is never answered here and shows nothing of what it asks`() {
        val q = question(secret = true)
        val a = QuestionDock.answersFor(q, null)
        assertEquals(QuestionField.SECRET, QuestionDock.field(q, a))
        assertNull(QuestionDock.ask(q, a))
        assertEquals("", QuestionDock.otherText(q, a))
        assertEquals(a, QuestionDock.typeOther(q, a, "hunter2"))
        assertFalse(QuestionDock.canSend(q, a, held = false))
        assertNull(QuestionDock.answersOut(q, a))
    }

    @Test
    fun `answers start empty for another question, and stay for the same one`() {
        val q1 = question(ask("0"), token = "q1")
        val typed = QuestionDock.typeOther(q1, QuestionDock.answersFor(q1, null), "words")
        assertEquals(typed, QuestionDock.answersFor(q1, typed))
        val q2 = question(ask("0"), token = "q2")
        assertEquals(QuestionAnswers("q2"), QuestionDock.answersFor(q2, typed))
    }

    // ---- Model, effort and mode ----

    private val claude = listOf(
        ModelOption(id = "default", name = "Default (recommended)", isDefault = true, efforts = listOf("low", "high"), defaultEffort = "high", images = true, short = true),
        ModelOption(id = "opus", name = "Opus", efforts = listOf("low", "medium", "high", "xhigh", "max"), images = true, short = true),
        ModelOption(id = "haiku", name = "Haiku", efforts = emptyList(), images = true, short = true),
        ModelOption(id = "claude-opus-4-1", name = "Opus 4.1", efforts = listOf("low", "high"), images = true),
        ModelOption(id = "claude-text-only", name = "Text", images = false),
    )

    private fun abilities(vararg can: String, models: List<ModelOption> = claude) =
        AgentAbilities(models = models, can = can.toSet(), control = true)

    @Test
    fun `the sheet lists short names first, the rest under More models, and never Claude's own default twice`() {
        val sections = AgentSettings.sections(claude)
        assertEquals(listOf("opus", "haiku"), sections.first.map { it.id })
        assertEquals(listOf("claude-opus-4-1", "claude-text-only"), sections.more.map { it.id })
        // Codex reports no short names: all of them are listed.
        val codex = listOf(ModelOption("gpt-5", "GPT-5", isDefault = true), ModelOption("gpt-5-mini", "GPT-5 mini"))
        assertEquals(ModelSections(codex, emptyList()), AgentSettings.sections(codex))
        assertEquals("GPT-5", AgentSettings.defaultModel(codex)?.name)
    }

    @Test
    fun `efforts follow the chosen model, and a choice it doesn't offer is dropped`() {
        var s = AgentSettings.pickModel(claude, ThreadSettings(), "opus")
        s = AgentSettings.pickEffort(claude, s, "xhigh")
        assertEquals("xhigh", s.effort)
        assertEquals("Xhigh", AgentSettings.effortLabel(s.effort!!))
        // Opus 4.1 has no xhigh.
        s = AgentSettings.pickModel(claude, s, "claude-opus-4-1")
        assertNull(s.effort)
        assertEquals("high", AgentSettings.effectiveEffort(claude, s.copy(effort = null).let { AgentSettings.pickModel(claude, it, null) }))
        s = AgentSettings.pickEffort(claude, s, "max")
        assertNull("not one of its efforts", s.effort)
        assertTrue(AgentSettings.efforts(claude, AgentSettings.pickModel(claude, s, "haiku")).isEmpty())
        // Claude's "default" entry is the sheet's Default row.
        assertNull(AgentSettings.pickModel(claude, s, "default").model)
        assertNull(AgentSettings.modelName(claude, null))
        assertEquals("Opus", AgentSettings.modelName(claude, "opus"))
        assertEquals("my-model", AgentSettings.modelName(claude, "my-model"))
    }

    @Test
    fun `a typed model is one argument, never a flag (contract Q7)`() {
        assertTrue(AgentSettings.isModelId("claude-sonnet-4-5"))
        assertTrue(AgentSettings.isModelId("a".repeat(64)))
        assertFalse(AgentSettings.isModelId("a".repeat(65)))
        assertFalse(AgentSettings.isModelId("--dangerously-skip-permissions"))
        assertFalse(AgentSettings.isModelId("-m"))
        assertFalse(AgentSettings.isModelId("opus haiku"))
        assertFalse(AgentSettings.isModelId("opus\n"))
        assertFalse(AgentSettings.isModelId("opus‮"))
        assertFalse(AgentSettings.isModelId(""))
        val typed = AgentSettings.typeModel(ThreadSettings(effort = "high"), "  my-model  ")
        assertEquals(ThreadSettings(model = "my-model", effort = null, customModel = true), typed)
        assertNull(AgentSettings.typeModel(ThreadSettings(), "-x"))
    }

    @Test
    fun `the phone offers two modes, Plan only when the agent has it`() {
        assertEquals(listOf(ThreadMode.SUPERVISED, ThreadMode.PLAN), AgentSettings.phoneModes(abilities(AgentAbilities.PLAN)))
        assertEquals(listOf(ThreadMode.SUPERVISED), AgentSettings.phoneModes(abilities()))
        // A PC without control offers nothing of it, whatever it says.
        assertEquals(listOf(ThreadMode.SUPERVISED), AgentSettings.phoneModes(AgentAbilities(can = setOf(AgentAbilities.PLAN), control = false)))
        assertEquals(ThreadSettings(), AgentSettings.pickMode(abilities(), ThreadSettings(), ThreadMode.AUTO_EDIT))
        assertEquals(ThreadSettings(), AgentSettings.pickMode(abilities(), ThreadSettings(), ThreadMode.PLAN))
        assertEquals(ThreadMode.PLAN, AgentSettings.pickMode(abilities(AgentAbilities.PLAN), ThreadSettings(), ThreadMode.PLAN).mode)
    }

    @Test
    fun `configure sends only what changed, an empty value for back to the default, never auto-edit`() {
        val now = ThreadSettings(model = "opus", effort = "high")
        assertNull(AgentSettings.change(now, now))
        assertEquals(ConfigureChange(model = null, effort = "max", mode = null, customModel = false), AgentSettings.change(now, now.copy(effort = "max")))
        assertEquals(ConfigureChange(model = "", effort = "", mode = null, customModel = false), AgentSettings.change(now, ThreadSettings()))
        assertEquals(ConfigureChange(model = null, effort = null, mode = ThreadMode.PLAN, customModel = false), AgentSettings.change(now, now.copy(mode = ThreadMode.PLAN)))
        assertNull(AgentSettings.change(now, now.copy(mode = ThreadMode.AUTO_EDIT)))
        assertEquals(
            ConfigureChange(model = "my-model", effort = "", mode = null, customModel = true),
            AgentSettings.change(now, ThreadSettings(model = "my-model", customModel = true)),
        )
    }

    @Test
    fun `images are offered with a model that takes them, and an unknown model when the agent does`() {
        assertTrue(AgentSettings.takesImages(abilities(AgentAbilities.IMAGES), ThreadSettings(model = "opus")))
        assertFalse(AgentSettings.takesImages(abilities(AgentAbilities.IMAGES), ThreadSettings(model = "claude-text-only")))
        assertTrue(AgentSettings.takesImages(abilities(AgentAbilities.IMAGES), ThreadSettings(model = "typed", customModel = true)))
        assertFalse(AgentSettings.takesImages(abilities(), ThreadSettings(model = "opus")))
    }

    @Test
    fun `the model chosen last is kept per agent and folder, without the mode`() {
        AgentChoices.forgetPc("pc-choices")
        AgentChoices.set("pc-choices", "claude", "g7linux", ThreadSettings(model = "opus", effort = "high", mode = ThreadMode.PLAN))
        assertEquals(ThreadSettings(model = "opus", effort = "high"), AgentChoices.get("pc-choices", "claude", "g7linux"))
        assertEquals(ThreadSettings(), AgentChoices.get("pc-choices", "codex", "g7linux"))
        AgentChoices.forgetPc("pc-choices")
        assertEquals(ThreadSettings(), AgentChoices.get("pc-choices", "claude", "g7linux"))
    }

    // ---- Sending while it works, and the queue ----

    @Test
    fun `steer by default when the agent can, queue when it can't, nothing from an older PC`() {
        assertEquals(FollowUp.STEER, AgentFollowUp.primary(abilities(AgentAbilities.STEER)))
        assertEquals(FollowUp.QUEUE, AgentFollowUp.other(FollowUp.STEER))
        assertEquals(FollowUp.QUEUE, AgentFollowUp.primary(abilities(AgentAbilities.IMAGES)))
        assertNull("no long-press when only queueing", AgentFollowUp.other(FollowUp.QUEUE))
        assertNull(AgentFollowUp.primary(AgentAbilities()))
    }

    @Test
    fun `a queued message cut short isn't edited here, and Send now waits for an answer`() {
        assertTrue(AgentQueue.canEdit(QueuedLine(id = "0123456789abcdef", text = "Then bump versionCode")))
        assertFalse(AgentQueue.canEdit(QueuedLine(id = "0123456789abcdef", text = "Long…", truncated = true)))
        assertTrue(AgentQueue.canSendNow(waiting = false))
        assertFalse(AgentQueue.canSendNow(waiting = true))
        val four = ThreadControl(queued = List(4) { QueuedLine(id = "000000000000000$it", text = "m$it") })
        assertTrue(AgentQueue.hasRoom(four))
        assertFalse(AgentQueue.hasRoom(four.copy(queued = four.queued + QueuedLine(id = "0000000000000009", text = "m9"))))
    }

    // ---- Images ----

    @Test
    fun `an image is told by its first bytes, within the wire's size`() {
        val png = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0)
        val jpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0xE0.toByte())
        val gif = "GIF89a".toByteArray()
        val webp = "RIFF\u0000\u0000\u0000\u0000WEBPVP8 ".toByteArray(Charsets.ISO_8859_1)
        val heic = byteArrayOf(0, 0, 0, 0x18, 0x66, 0x74, 0x79, 0x70, 0x68, 0x65, 0x69, 0x63)
        assertEquals(ImageKind.PNG, AgentImages.kind(png))
        assertEquals(ImageKind.JPEG, AgentImages.kind(jpeg))
        assertEquals(ImageKind.GIF, AgentImages.kind(gif))
        assertEquals(ImageKind.WEBP, AgentImages.kind(webp))
        assertNull(AgentImages.kind(heic))
        assertNull(AgentImages.kind(ByteArray(0)))
        assertEquals(ImageProblem.NOT_AN_IMAGE, AgentImages.problem(heic))
        assertNull(AgentImages.problem(png))
        assertEquals(ImageProblem.TOO_LARGE, AgentImages.problem(png + ByteArray(AgentImages.MAX_BYTES)))
        assertTrue(AgentImages.isThumb(jpeg))
        assertFalse("a thumbnail is a JPEG", AgentImages.isThumb(png))
        assertFalse(AgentImages.isThumb(jpeg + ByteArray(AgentImages.MAX_THUMB_BYTES)))
    }

    @Test
    fun `up to four images a message, and one removed while it uploads stays removed`() {
        var images = emptyList<PendingImage>()
        for (key in listOf("a", "b", "c", "d", "e")) images = AgentImages.adding(images, key)
        assertEquals(listOf("a", "b", "c", "d"), images.map { it.key })
        assertEquals(0, AgentImages.room(images))
        assertTrue(AgentImages.uploading(images))
        images = AgentImages.uploaded(images, "a", "f".repeat(32))
        images = AgentImages.removed(images, "b")
        images = AgentImages.uploaded(images, "b", "e".repeat(32))
        assertEquals(listOf("a", "c", "d"), images.map { it.key })
        assertEquals(listOf("f".repeat(32)), AgentImages.readyIds(images))
        images = AgentImages.uploaded(AgentImages.uploaded(images, "c", "1".repeat(32)), "d", "2".repeat(32))
        assertFalse(AgentImages.uploading(images))
        assertEquals(1, AgentImages.room(images))
    }

    // ---- Context and limits ----

    @Test
    fun `context reads as a share of the window, the window in K and M`() {
        assertEquals(22, AgentUsage.contextPercent(ContextUse(used = 104_500, max = 475_000)))
        assertEquals(100, AgentUsage.contextPercent(ContextUse(used = 600_000, max = 475_000)))
        assertNull(AgentUsage.contextPercent(ContextUse(used = 1, max = 0)))
        assertEquals("475K", AgentUsage.tokens(475_000))
        assertEquals("200K", AgentUsage.tokens(200_000))
        assertEquals("1M", AgentUsage.tokens(1_000_000))
        assertEquals("1M", AgentUsage.tokens(1_048_576))
        assertEquals("1.5M", AgentUsage.tokens(1_500_000))
        assertEquals("950", AgentUsage.tokens(950))
    }

    @Test
    fun `windows are worded by the client, and one line shows the worst limit`() {
        assertEquals(WindowName.FIVE_HOUR, AgentUsage.windowName("300"))
        assertEquals(WindowName.FIVE_HOUR, AgentUsage.windowName("five_hour"))
        assertEquals(WindowName.WEEKLY, AgentUsage.windowName("10080"))
        assertEquals(WindowName.WEEKLY, AgentUsage.windowName("seven_day"))
        assertEquals(WindowName.AS_GIVEN, AgentUsage.windowName("opus_weekly"))
        val ok = LimitInfo("five_hour", 41, 1L, LimitState.OK)
        val warn = LimitInfo("seven_day", 85, 2L, LimitState.WARN)
        val reached = LimitInfo("five_hour", 100, 3L, LimitState.REACHED)
        assertNull(AgentUsage.pinned(listOf(ok)))
        assertEquals(warn, AgentUsage.pinned(listOf(ok, warn)))
        assertEquals(reached, AgentUsage.pinned(listOf(warn, reached)))
    }

    @Test
    fun `the MCP line shows a count only when the folder's switch is on and servers were reported`() {
        assertTrue(AgentUsage.mcpShown(McpCount(on = true, servers = 4, connected = 3, needLogin = 1)))
        assertFalse(AgentUsage.mcpShown(McpCount(on = false, servers = 4, connected = 3, needLogin = 1)))
        assertFalse(AgentUsage.mcpShown(McpCount(on = true, servers = 0, connected = 0, needLogin = 0)))
        assertFalse(AgentUsage.mcpShown(null))
    }
}
