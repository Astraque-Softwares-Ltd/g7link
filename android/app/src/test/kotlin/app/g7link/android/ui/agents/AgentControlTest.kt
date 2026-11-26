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
        // Codex reports no short names: all of them are listed, its default once (as the Default row, the same row again under More).
        val codex = listOf(ModelOption("gpt-5", "GPT-5", isDefault = true), ModelOption("gpt-5-mini", "GPT-5 mini"))
        assertEquals(ModelSections(listOf(codex[1]), listOf(codex[0])), AgentSettings.sections(codex))
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
    fun `configure sends only what changed, an empty value for back to the default`() {
        val now = ThreadSettings(model = "opus", effort = "high")
        assertNull(AgentSettings.change(now, now))
        assertEquals(ConfigureChange(model = null, effort = "max", mode = null, customModel = false), AgentSettings.change(now, now.copy(effort = "max")))
        assertEquals(ConfigureChange(model = "", effort = "", mode = null, customModel = false), AgentSettings.change(now, ThreadSettings()))
        assertEquals(ConfigureChange(model = null, effort = null, mode = ThreadMode.PLAN, customModel = false), AgentSettings.change(now, now.copy(mode = ThreadMode.PLAN)))
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

    // ---- Exact names and quick picks (contract C §2, §3) ----

    /** Claude as a CLI with `resolvedModel` reports it: every alias knows its exact model. */
    private val opus5 = listOf(
        ModelOption("default", "Default (recommended)", isDefault = true, efforts = listOf("low", "medium", "high", "xhigh", "max"), images = true, short = true, exact = "Opus 5"),
        ModelOption("opus", "Opus", efforts = listOf("low", "medium", "high", "xhigh", "max"), images = true, short = true, exact = "Opus 5"),
        ModelOption("sonnet", "Sonnet", efforts = listOf("low", "high"), images = true, short = true, exact = "Sonnet 4.6"),
        ModelOption("haiku", "Haiku", images = true, short = true, exact = "Haiku 4.5"),
        ModelOption("claude-opus-5", "Opus 5", efforts = listOf("low", "medium", "high", "xhigh", "max"), images = true, exact = "Opus 5"),
    )

    private val gpt = listOf(
        ModelOption("gpt-5.5-codex", "GPT-5.5 Codex", isDefault = true, efforts = listOf("low", "medium", "high", "xhigh"), defaultEffort = "medium", exact = "GPT-5.5 Codex"),
        ModelOption("gpt-5.4-mini", "GPT-5.4 mini", efforts = listOf("low", "medium"), defaultEffort = "low", exact = "GPT-5.4 mini"),
    )

    private val claudeAgent = PickAgent("claude", "Claude", ready = true, models = opus5)
    private val codexAgent = PickAgent("codex", "Codex", ready = true, models = gpt)

    private fun used(id: String, ms: Long, agent: String? = null, model: String? = null, effort: String? = null) =
        FolderUse(id = id, label = "label-$id", usedMs = ms, last = agent?.let { FolderLast(it, model, effort, title = "Fix it", branch = "main") })

    @Test
    fun `a model is shown by its exact name, and the default by its own without a word for it`() {
        assertEquals("Opus 5", AgentSettings.shownName(opus5[1]))
        assertEquals("Opus 5", AgentSettings.modelName(opus5, "opus"))
        assertEquals("Opus 5", AgentSettings.settingName(opus5, null))
        assertEquals("Sonnet 4.6", AgentSettings.settingName(opus5, "sonnet"))
        // No model set: what answered there when the list has it, else the default's.
        assertEquals("Sonnet 4.6", AgentSettings.settingName(opus5, null, answered = "sonnet"))
        assertEquals("Opus 5", AgentSettings.settingName(opus5, null, answered = "claude-gone-1"))
        assertEquals("my-model", AgentSettings.settingName(opus5, "my-model"))
        assertEquals("GPT-5.5 Codex", AgentSettings.settingName(gpt, null))
    }

    @Test
    fun `without the exact model the alias keeps its name and the default isn't guessed`() {
        // `claude` above is a CLI that doesn't say which model an alias stands for.
        assertNull(AgentSettings.shownName(claude[0]))
        assertNull(AgentSettings.settingName(claude, null))
        assertEquals("Opus", AgentSettings.settingName(claude, "opus"))
        assertNull(AgentSettings.pickName(claude, RunChoice("claude")))
        // Nothing reported at all.
        assertNull(AgentSettings.settingName(emptyList(), null))
        // Codex's default is a real model with its own name.
        assertEquals("GPT-5", AgentSettings.shownName(ModelOption("gpt-5", "GPT-5", isDefault = true)))
    }

    @Test
    fun `rows that read the same are shown once up top, all of them under More with their ids`() {
        val sections = AgentSettings.sections(opus5)
        // "Opus 5" is the Default row's: the opus alias moves under More.
        assertEquals(listOf("sonnet", "haiku"), sections.first.map { it.id })
        assertEquals(listOf("opus", "claude-opus-5"), sections.more.map { it.id })
        assertEquals(setOf("opus", "claude-opus-5"), AgentSettings.sameNamed(opus5))
        assertTrue(AgentSettings.sameNamed(claude).isEmpty())
    }

    @Test
    fun `quick picks are this folder's last, then other folders' by use, then the defaults, three at most`() {
        val folders = listOf(
            used("here", 100L, "claude", "opus", "high"),
            used("old", 50L, "claude", "haiku"),
            used("newest", 300L, "codex", "gpt-5.4-mini", "low"),
            used("middle", 200L, "claude", "sonnet", "high"),
        )
        val picks = AgentSettings.quickPicks(folders, listOf(claudeAgent, codexAgent), "here")
        assertEquals(
            listOf(
                QuickPick(RunChoice("claude", "opus", "high"), PickFrom.HERE),
                QuickPick(RunChoice("codex", "gpt-5.4-mini", "low"), PickFrom.FOLDER, folder = "label-newest"),
                QuickPick(RunChoice("claude", "sonnet", "high"), PickFrom.FOLDER, folder = "label-middle"),
            ),
            picks,
        )
        assertEquals(AgentSettings.MAX_PICKS, picks.size)
    }

    @Test
    fun `the same pick from two folders is shown once, and the agents' defaults fill the rest`() {
        val folders = listOf(used("here", 100L, "claude", "opus", "high"), used("other", 200L, "claude", "opus", "high"), used("new", 0L))
        val picks = AgentSettings.quickPicks(folders, listOf(claudeAgent, codexAgent), "here")
        assertEquals(listOf(PickFrom.HERE, PickFrom.DEFAULT), picks.map { it.from })
        assertEquals(RunChoice("codex"), picks[1].choice)
        // No history at all: one default per ready agent, in the catalog's order.
        assertEquals(
            listOf(QuickPick(RunChoice("claude"), PickFrom.DEFAULT), QuickPick(RunChoice("codex"), PickFrom.DEFAULT)),
            AgentSettings.quickPicks(listOf(used("new", 0L)), listOf(claudeAgent, codexAgent), "new"),
        )
    }

    @Test
    fun `an agent that can't start a thread gives no pick and a line says why`() {
        val signedOut = codexAgent.copy(ready = false)
        val folders = listOf(used("here", 100L, "codex", "gpt-5.4-mini"), used("other", 90L, "claude", "sonnet"))
        val picks = AgentSettings.quickPicks(folders, listOf(claudeAgent, signedOut), "here")
        assertEquals(listOf("claude"), picks.map { it.choice.agent }.distinct())
        assertEquals(PickFrom.FOLDER, picks.first().from)
        assertEquals(listOf(signedOut), AgentSettings.leftOut(folders, listOf(claudeAgent, signedOut)))
        assertTrue(AgentSettings.leftOut(folders, listOf(claudeAgent, codexAgent)).isEmpty())
    }

    @Test
    fun `a model gone from the list is still offered, sent as typed, and an effort it no longer takes is dropped`() {
        val folders = listOf(used("here", 100L, "claude", "claude-opus-4-1", "high"), used("b", 90L, "claude", "sonnet", "max"))
        val picks = AgentSettings.quickPicks(folders, listOf(claudeAgent), "here")
        val gone = picks[0].choice
        assertEquals(RunChoice("claude", "claude-opus-4-1", "high", custom = true), gone)
        assertTrue(AgentSettings.delisted(opus5, gone))
        assertEquals("claude-opus-4-1", AgentSettings.pickName(opus5, gone))
        assertEquals(ThreadSettings(model = "claude-opus-4-1", effort = "high", customModel = true), gone.settings())
        // Sonnet takes low and high only.
        assertEquals(RunChoice("claude", "sonnet", null), picks[1].choice)
        assertFalse(AgentSettings.delisted(opus5, picks[1].choice))
        // The list's default by name is no model set.
        assertEquals(RunChoice("codex", null, "high"), AgentSettings.normalized(codexAgent, "gpt-5.5-codex", "high"))
        assertEquals(RunChoice("claude", null, "max"), AgentSettings.normalized(claudeAgent, "default", "max"))
    }

    @Test
    fun `a pick reads agent, exact model and effort, the model's own effort when none is set`() {
        val claudeDefault = RunChoice("claude")
        assertEquals("Opus 5", AgentSettings.pickName(opus5, claudeDefault))
        assertNull("Claude reports no default effort", AgentSettings.pickEffort(opus5, claudeDefault))
        assertEquals("High", AgentSettings.pickEffort(opus5, RunChoice("claude", "opus", "high")))
        assertEquals("GPT-5.5 Codex", AgentSettings.pickName(gpt, RunChoice("codex")))
        assertEquals("Medium", AgentSettings.pickEffort(gpt, RunChoice("codex")))
    }

    @Test
    fun `Run with selects the first pick, a chosen one, or adds the choice as Your choice`() {
        val picks = listOf(QuickPick(RunChoice("claude", "opus", "high"), PickFrom.HERE), QuickPick(RunChoice("codex"), PickFrom.DEFAULT))
        assertEquals(listOf(true, false), AgentSettings.runRows(picks, null).map { it.selected })
        assertEquals(listOf(false, true), AgentSettings.runRows(picks, RunChoice("codex")).map { it.selected })
        val yours = RunChoice("claude", "sonnet", "low")
        val rows = AgentSettings.runRows(picks, yours)
        assertEquals(listOf(false, false, true), rows.map { it.selected })
        assertEquals(QuickPick(yours, PickFrom.YOURS), rows.last().pick)
        assertEquals(yours, AgentSettings.runWith(picks, yours))
        assertEquals(picks[0].choice, AgentSettings.runWith(picks, null))
        assertNull(AgentSettings.runWith(emptyList(), null))
        // The sheet's choice, as a start sends it: Codex's default model by name is no model set; a typed one stays typed.
        assertEquals(RunChoice("codex", null, "low"), AgentSettings.choiceOf(codexAgent, ThreadSettings(model = "gpt-5.5-codex", effort = "low")))
        assertEquals(RunChoice("claude", "opus", null, custom = true), AgentSettings.choiceOf(claudeAgent, ThreadSettings(model = "opus", customModel = true)))
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
