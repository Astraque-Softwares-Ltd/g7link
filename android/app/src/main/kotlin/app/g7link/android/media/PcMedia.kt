// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.VolumeProvider
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.core.content.ContextCompat
import app.g7link.android.G7LinkApplication
import app.g7link.android.R
import app.g7link.android.ui.MainActivity
import app.g7link.core.G7LinkNode
import app.g7link.core.MediaAction
import app.g7link.core.MediaPlayer
import app.g7link.core.MediaRepeat
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.consumeEach
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * What plays on a paired PC, in Android's own media controls (the
 * notification shade and Quick Settings player): a media session that
 * stands for the PC's player, and its notification. What the user presses
 * goes to the PC through [send]; the PC's new state comes back as an update.
 *
 * While a PC player plays and [volumeKeys] is on, the session's volume is
 * the PC's: Android then sends the phone's volume buttons to it (unless
 * the phone plays something itself), and [setPcAudio] passes them on.
 *
 * It also feeds the app's own player (ui/player): every PC player's
 * [Summary] in [everyPlayer], where each one is in [timelines], the PCs'
 * volume in [pcLevels], and the large artwork and app icons in [pictures],
 * fetched through [node].
 *
 * Applies each PC's newest snapshot, in arrival order, and decodes artwork
 * off the main thread.
 */
class PcMedia(
    context: Context,
    private val scope: CoroutineScope,
    private val onSeedColorChanged: () -> Unit = {},
    private val setPcAudio: (pc: String, volume: Int?, muted: Boolean?) -> Unit = { _, _, _ -> },
    /**
     * The running node, waiting for it to start (null while there's none), for the large
     * artwork, app icons, shuffle and repeat. Without it those aren't offered.
     */
    private val node: (suspend () -> G7LinkNode?)? = null,
    private val send: (pc: String, player: String, action: MediaAction, position: ULong?) -> Unit,
) {
    private val context = context.applicationContext
    private val main = Handler(Looper.getMainLooper())

    /** Players per PC, in the PC's order, and the PCs' names. */
    private val players = linkedMapOf<String, List<MediaPlayer>>()
    private val names = mutableMapOf<String, String>()
    /** Decoded artwork and its colours by PC and key (artwork arrives once per key). */
    private val art = object : LinkedHashMap<String, Pair<Bitmap, ArtColors?>>(8, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Pair<Bitmap, ArtColors?>>) = size > 8
    }

    /**
     * One PC player as the app shows it. [subtitle] is "artist · app on PC";
     * [source] is "app on PC" alone (or the PC's name). The fields after
     * [artwork] are for the app's player and have defaults.
     */
    @androidx.compose.runtime.Immutable
    data class Summary(
        val playerId: String,
        val title: String,
        val subtitle: String,
        val playing: Boolean,
        val artwork: Bitmap?,
        val pc: String = "",
        val pcName: String = "",
        val appName: String = "",
        val source: String = "",
        val artist: String? = null,
        val album: String? = null,
        val artKey: String? = null,
        val artFullKey: String? = null,
        val iconKey: String? = null,
        val colors: ArtColors? = null,
        val shuffle: Boolean? = null,
        val repeat: MediaRepeat? = null,
        val canPlay: Boolean = false,
        val canPause: Boolean = false,
        val canNext: Boolean = false,
        val canPrevious: Boolean = false,
        val canSeek: Boolean = false,
        val canShuffle: Boolean = false,
        val canRepeat: Boolean = false,
        /** When it last started playing, changed track or appeared (`elapsedRealtime`). */
        val activeAt: Long = 0L,
    )

    @Volatile
    var activeSeedColor: UInt? = null
        private set

    private val _playing = MutableStateFlow<Map<String, Boolean>>(emptyMap())

    /** Whether each PC player plays, by "pc/player" (for Play on phone's controls). */
    val playing: StateFlow<Map<String, Boolean>> = _playing

    /**
     * Where a PC player is in what it plays: [positionMs] as of [atElapsedMs]
     * (`SystemClock.elapsedRealtime`), moving on at [rateMilli] thousandths
     * of real time while [playing].
     */
    data class Timeline(
        val positionMs: Long,
        val durationMs: Long,
        val playing: Boolean,
        val canSeek: Boolean,
        val atElapsedMs: Long,
        val rateMilli: Int = NORMAL_RATE,
    ) {
        /** Where it is now. */
        fun positionAt(elapsedMs: Long): Long {
            val start = positionMs.coerceIn(0, durationMs)
            if (!playing) return start
            val elapsed = (elapsedMs - atElapsedMs).coerceAtLeast(0L)
            val rate = rateMilli.coerceAtLeast(1)
            // Saturating, so a far-off clock or a huge duration never wraps round.
            val moved = if (elapsed > Long.MAX_VALUE / rate) Long.MAX_VALUE else elapsed * rate / NORMAL_RATE
            return if (moved >= durationMs - start) durationMs else start + moved
        }
    }

    /** A seek sent from here: [shown] at once, the PC's own positions ignored until it agrees or [at] is [SEEK_HOLD_MS] old. */
    internal data class SeekHold(val shown: Timeline, val at: Long)

    private val _timelines = MutableStateFlow<Map<String, Timeline>>(emptyMap())

    /** Each PC player's [Timeline] that has one, by "pc/player" (Play on phone's seek bar, the player). */
    val timelines: StateFlow<Map<String, Timeline>> = _timelines

    private val _summaries = MutableStateFlow<Map<String, Summary>>(emptyMap())

    /** Each PC's foremost player: the first that plays, else its first. */
    val summaries: StateFlow<Map<String, Summary>> = _summaries

    private val _everyPlayer = MutableStateFlow<List<Summary>>(emptyList())

    /** Every player on every PC, in the PCs' order (the app's player and its switcher). */
    val everyPlayer: StateFlow<List<Summary>> = _everyPlayer

    /** The large artwork and app icons the PCs' players name. */
    val pictures = PcArt(
        scope = scope,
        fetchArt = { pc, key -> node?.invoke()?.mediaArt(pc, key) },
        fetchIcon = { pc, key -> node?.invoke()?.mediaIcon(pc, key) },
        screenSide = this.context.resources.displayMetrics.let { minOf(it.widthPixels, it.heightPixels) },
    )

    private val waiting = SnapshotCoalescer<MediaPlayer>(artKeyOf = { it.artKey }, artOf = { it.art })
    private val wake = Channel<Unit>(Channel.CONFLATED)

    init {
        // The Rust listener delivers snapshots and disconnects in order; the
        // coalescer keeps that order. The next batch is taken only once the
        // main thread has applied the last, so a slow phone skips snapshots
        // rather than queueing them.
        scope.launch(Dispatchers.Default) {
            wake.consumeEach {
                val batch = waiting.take() ?: return@consumeEach
                applyBatch(batch)
            }
        }
    }

    private var session: MediaSession? = null
    private var shown: Pair<String, MediaPlayer>? = null

    /** Each PC's master volume, for the PCs whose volume this phone may set. */
    private var levels: Map<String, PcLevel> = emptyMap()
    private var volumeKeys = true
    /** The PC the session's volume stands for, while it does. */
    private var remote: PcVolume? = null

    private val _levels = MutableStateFlow<Map<String, PcLevel>>(emptyMap())

    /** The master volume of each PC this phone may set (the player's volume bar). */
    val pcLevels: StateFlow<Map<String, PcLevel>> = _levels

    private val _volumeKeys = MutableStateFlow(true)

    /** Whether the volume buttons set a playing PC's volume (Settings). */
    val volumeKeysOn: StateFlow<Boolean> = _volumeKeys

    /** A PC's master volume (`0..=100`) and mute. */
    data class PcLevel(val volume: Int, val muted: Boolean)
    private val stalePauseTimeout = Runnable { hide() }

    /** Each PC's summary as the PC last described it, before any play or pause pressed here. */
    private var rendered: Map<String, Summary> = emptyMap()
    private var renderedAll: List<Summary> = emptyList()

    /** Puts back what the PC said when it never answered a play or pause. */
    private val undoPressedPlaying = Runnable {
        if (_everyPlayer.value != renderedAll) _everyPlayer.value = renderedAll
        if (_summaries.value != rendered) {
            _summaries.value = rendered
            app.g7link.android.widget.PcWidgetProvider.refreshAll(context)
        }
    }

    /** When each "pc/player" last became active (see [Summary.activeAt]). */
    private val activity = HashMap<String, Activity>()
    private class Activity(val playing: Boolean, val track: String, val at: Long)

    /** Seeks sent from here that the PC hasn't confirmed yet, by "pc/player". */
    private val seekHolds = HashMap<String, SeekHold>()

    /** Shuffle or repeat pressed here, shown until the PC agrees or [PRESSED_PLAYING_MS] passes. */
    private val toggles = HashMap<Pair<String, String>, Toggle>()
    private data class Toggle(val shuffle: Boolean?, val repeat: MediaRepeat?, val until: Long)
    private val expireToggles = Runnable { publish() }

    private val callback = object : MediaSession.Callback() {
        override fun onPlay() = command(MediaAction.PLAY)
        override fun onPause() = command(MediaAction.PAUSE)
        override fun onSkipToNext() = command(MediaAction.NEXT)
        override fun onSkipToPrevious() = command(MediaAction.PREVIOUS)
        override fun onSeekTo(pos: Long) {
            val (pc, player) = shown ?: return
            seek(pc, player.id, pos)
        }
    }

    /** A PC's players changed (empty: nothing plays there, or it's offline). */
    fun update(pc: String, pcName: String, list: List<MediaPlayer>) {
        waiting.offer(pc, pcName, list)
        wake.trySend(Unit)
    }

    private suspend fun applyBatch(batch: SnapshotCoalescer.Batch<MediaPlayer>) {
        val decoded = batch.art.mapNotNull { (cacheKey, bytes) ->
            val bmp = decodeArtwork(bytes) ?: return@mapNotNull null
            cacheKey to (bmp to artColors(bmp, cacheKey))
        }
        val applied = CompletableDeferred<Unit>()
        main.post {
            for ((cacheKey, entry) in decoded) {
                art[cacheKey] = entry
            }
            for (update in batch.updates) {
                names[update.pc] = update.name
                if (update.reattach) players.remove(update.pc)
                if (update.players.isEmpty()) players.remove(update.pc) else players[update.pc] = update.players
            }
            val now = SystemClock.elapsedRealtime()
            noteActivity(now)
            _playing.value = players.flatMap { (id, all) -> all.map { "$id/${it.id}" to it.playing } }.toMap()
            _timelines.value = players.flatMap { (id, all) ->
                all.mapNotNull { p ->
                    val key = "$id/${p.id}"
                    val incoming = timelineOf(p, now) ?: return@mapNotNull null
                    key to settle(key, incoming, now)
                }
            }.toMap()
            seekHolds.keys.retainAll(_timelines.value.keys)
            render()
            applied.complete(Unit)
        }
        applied.await()
    }

    /** [incoming] unless a seek from here still holds for [key]. */
    private fun settle(key: String, incoming: Timeline, now: Long): Timeline {
        val hold = seekHolds[key] ?: return incoming
        val (timeline, keep) = settled(incoming, hold, now)
        if (!keep) seekHolds.remove(key)
        return timeline
    }

    /** Notes which players started, changed track or appeared, for "the latest" (see [Summary.activeAt]). */
    private fun noteActivity(now: Long) {
        val present = HashSet<String>()
        for ((pc, list) in players) {
            for (p in list) {
                val key = "$pc/${p.id}"
                present += key
                val track = "${p.title}\u0001${p.artKey}"
                val before = activity[key]
                if (before == null || (p.playing && !before.playing) || before.track != track) {
                    activity[key] = Activity(p.playing, track, now)
                } else if (before.playing != p.playing) {
                    activity[key] = Activity(p.playing, track, before.at)
                }
            }
        }
        activity.keys.retainAll(present)
    }

    /** The volume of each PC this phone may set (from its deck state). */
    fun levels(levels: Map<String, PcLevel>) = main.post {
        if (levels == this.levels) return@post
        this.levels = levels
        _levels.value = levels
        session?.let { route(it) }
    }

    /** Whether the volume buttons may set a playing PC's volume (Settings). */
    fun volumeKeys(enabled: Boolean) = main.post {
        if (enabled == volumeKeys) return@post
        volumeKeys = enabled
        _volumeKeys.value = enabled
        session?.let { route(it) }
    }

    /** Sets PC [pc]'s master volume (`0..=100`) and/or mute (the player's volume bar). */
    fun setVolume(pc: String, volume: Int?, muted: Boolean?) = setPcAudio(pc, volume?.coerceIn(0, 100), muted)

    /** Asks the PC [pc]'s [player] to [action] (Play on phone's controls). */
    fun command(pc: String, player: String, action: MediaAction) = send(pc, player, action, null)

    /**
     * Asks the PC [pc]'s [player] to go to [positionMs] (Play on phone's seek bar, the player):
     * one seek, shown at once, and the PC's positions are ignored for [SEEK_HOLD_MS] unless
     * they're within [POSITION_SLACK_MS] of it.
     */
    fun seek(pc: String, player: String, positionMs: Long) {
        val target = positionMs.coerceAtLeast(0)
        send(pc, player, MediaAction.SEEK, target.toULong())
        main.post {
            val key = "$pc/$player"
            val current = _timelines.value[key] ?: return@post
            val now = SystemClock.elapsedRealtime()
            val held = current.copy(positionMs = target.coerceIn(0, current.durationMs), atElapsedMs = now)
            seekHolds[key] = SeekHold(held, now)
            _timelines.value = _timelines.value + (key to held)
        }
    }

    /** Turns PC [pc]'s [player]'s shuffle on or off (`media.command` `shuffle`). */
    fun setShuffle(pc: String, player: String, on: Boolean) = main.post {
        sendValue(pc, player, MediaAction.SHUFFLE, if (on) SHUFFLE_ON else SHUFFLE_OFF)
        toggle(pc, player) { it.copy(shuffle = on) }
    }

    /** Sets PC [pc]'s [player]'s repeat (`media.command` `loop`). */
    fun setRepeat(pc: String, player: String, repeat: MediaRepeat) = main.post {
        sendValue(pc, player, MediaAction.REPEAT, repeatValue(repeat))
        toggle(pc, player) { it.copy(repeat = repeat) }
    }

    private fun toggle(pc: String, player: String, change: (Toggle) -> Toggle) {
        val now = SystemClock.elapsedRealtime()
        val key = pc to player
        val before = toggles[key]?.takeIf { it.until > now } ?: Toggle(null, null, 0L)
        toggles[key] = change(before).copy(until = now + PRESSED_PLAYING_MS)
        publish()
        main.removeCallbacks(expireToggles)
        main.postDelayed(expireToggles, PRESSED_PLAYING_MS + 1)
    }

    private fun sendValue(pc: String, player: String, action: MediaAction, value: String) {
        scope.launch {
            val running = node?.invoke() ?: return@launch
            try {
                running.mediaCommandValue(pc, player, action, value)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.i(TAG, "a media command wasn't taken: ${e.javaClass.simpleName}")
            }
        }
    }

    private fun command(action: MediaAction, position: ULong? = null) {
        val (pc, player) = shown ?: return
        send(pc, player.id, action, position)
    }

    private fun summary(pc: String, player: MediaPlayer, now: Long): Summary {
        val pcName = names[pc].orEmpty().ifBlank { context.getString(R.string.your_pc) }
        val cleanApp = sanitizeAppName(player.app)
        val cleanTitle = player.title?.trim()?.takeIf { it.isNotEmpty() } ?: cleanApp.ifEmpty { pcName }
        val cleanArtist = player.artist?.let(::sanitizeArtistLabel)?.takeIf {
            it.isNotEmpty() && !it.equals(cleanApp, ignoreCase = true) && !it.equals(cleanTitle, ignoreCase = true)
        }
        val titleAlreadyMentionsApp = cleanApp.isNotEmpty() &&
            cleanTitle.endsWith(" on $cleanApp", ignoreCase = true)
        val where = if (cleanApp.isEmpty() ||
            titleAlreadyMentionsApp ||
            cleanApp.equals("Browser", ignoreCase = true) ||
            cleanApp.equals("Media", ignoreCase = true)
        ) {
            pcName
        } else {
            context.getString(R.string.media_on, cleanApp, pcName)
        }
        val subtitleLine = listOfNotNull(cleanArtist, where).distinct().joinToString(" · ")
        val picture = player.artKey?.let { art["$pc/$it"] }
        val toggle = toggles[pc to player.id]?.takeIf { it.until > now }

        return Summary(
            playerId = player.id,
            title = cleanTitle,
            subtitle = subtitleLine,
            playing = player.playing,
            artwork = picture?.first,
            pc = pc,
            pcName = pcName,
            appName = cleanApp,
            source = where,
            artist = cleanArtist,
            album = player.album?.trim()?.takeIf { it.isNotEmpty() },
            artKey = player.artKey,
            artFullKey = player.artFull?.takeIf { PcArt.isArtKey(it) },
            iconKey = player.icon?.takeIf { PcArt.isIconKey(it) },
            colors = picture?.second,
            shuffle = toggle?.shuffle ?: player.shuffle,
            repeat = toggle?.repeat ?: player.repeat,
            canPlay = MediaAction.PLAY in player.actions,
            canPause = MediaAction.PAUSE in player.actions,
            canNext = MediaAction.NEXT in player.actions,
            canPrevious = MediaAction.PREVIOUS in player.actions,
            canSeek = MediaAction.SEEK in player.actions && (player.duration?.toLong() ?: 0L) > 0L,
            canShuffle = node != null && MediaAction.SHUFFLE in player.actions && player.shuffle != null,
            canRepeat = node != null && MediaAction.REPEAT in player.actions && player.repeat != null,
            activeAt = activity["$pc/${player.id}"]?.at ?: 0L,
        )
    }

    /** Puts every player's summary out: [everyPlayer], and each PC's foremost in [summaries]. */
    private fun publish() {
        val now = SystemClock.elapsedRealtime()
        toggles.entries.removeAll { (key, toggle) ->
            val player = players[key.first]?.firstOrNull { it.id == key.second }
            toggle.until <= now ||
                player == null ||
                ((toggle.shuffle == null || toggle.shuffle == player.shuffle) && (toggle.repeat == null || toggle.repeat == player.repeat))
        }
        val all = players.flatMap { (pc, list) -> list.map { summary(pc, it, now) } }
        val foremost = LinkedHashMap<String, Summary>()
        for (each in all) {
            val current = foremost[each.pc]
            if (current == null || (!current.playing && each.playing)) foremost[each.pc] = each
        }
        rendered = foremost
        renderedAll = all
        if (_everyPlayer.value != all) _everyPlayer.value = all
        if (_summaries.value != foremost) {
            _summaries.value = foremost
            app.g7link.android.widget.PcWidgetProvider.refreshAll(context)
        }
    }

    private fun render() {
        main.removeCallbacks(stalePauseTimeout)
        main.removeCallbacks(undoPressedPlaying)
        publish()
        val nextSummaries = rendered

        val preferred = players.mapValues { (_, list) -> list.firstOrNull { it.playing } ?: list.first() }
        val first = preferred.entries.firstOrNull { it.value.playing } ?: preferred.entries.firstOrNull()
        if (first == null) return hide()
        val (pc, player) = first
        shown = pc to player
        val session = session ?: MediaSession(context, "G7 Link PC media").also {
            it.setCallback(callback, main)
            it.setSessionActivity(openApp())
            session = it
        }
        val nextSeed = player.artKey?.let { art["$pc/$it"]?.second?.scheme?.seedRgb }
        if (activeSeedColor != nextSeed) {
            activeSeedColor = nextSeed
            onSeedColorChanged()
        }
        val summary = nextSummaries.getValue(pc)
        val cleanTitle = summary.title
        val subtitleLine = summary.subtitle
        val picture = summary.artwork

        session.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, cleanTitle)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, subtitleLine)
                .putString(MediaMetadata.METADATA_KEY_ALBUM, player.album)
                .apply { player.duration?.let { putLong(MediaMetadata.METADATA_KEY_DURATION, it.toLong()) } }
                .apply { picture?.let { putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, it) } }
                .build(),
        )
        var actions = 0L
        if (MediaAction.PLAY in player.actions) actions = actions or PlaybackState.ACTION_PLAY
        if (MediaAction.PAUSE in player.actions) actions = actions or PlaybackState.ACTION_PAUSE
        if (MediaAction.PLAY in player.actions && MediaAction.PAUSE in player.actions) {
            actions = actions or PlaybackState.ACTION_PLAY_PAUSE
        }
        if (MediaAction.NEXT in player.actions) actions = actions or PlaybackState.ACTION_SKIP_TO_NEXT
        if (MediaAction.PREVIOUS in player.actions) actions = actions or PlaybackState.ACTION_SKIP_TO_PREVIOUS
        if (MediaAction.SEEK in player.actions) actions = actions or PlaybackState.ACTION_SEEK_TO
        session.setPlaybackState(
            PlaybackState.Builder()
                .setActions(actions)
                .setState(
                    if (player.playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                    player.position?.toLong() ?: PlaybackState.PLAYBACK_POSITION_UNKNOWN,
                    if (player.playing) rateOf(player) / NORMAL_RATE.toFloat() else 0f,
                    // When the PC sampled the position, not when it arrived.
                    SystemClock.elapsedRealtime() - ageOf(player),
                )
                .build(),
        )
        route(session)
        session.isActive = true
        post(notification(session, player, cleanTitle, subtitleLine, picture))
        if (!player.playing) {
            main.postDelayed(stalePauseTimeout, 600_000L)
        }
    }

    /**
     * Gives the session the shown PC's volume while that PC plays and its
     * volume is known, and the phone's media volume otherwise.
     */
    private fun route(session: MediaSession) {
        val (pc, player) = shown ?: return
        val level = levels[pc]?.takeIf { volumeKeys && player.playing }
        if (level == null) {
            if (remote != null) {
                remote = null
                session.setPlaybackToLocal(
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build(),
                )
            }
            return
        }
        val current = remote
        if (current != null && current.pc == pc) {
            current.show(level)
            return
        }
        remote = PcVolume(pc, level).also(session::setPlaybackToRemote)
    }

    /** The PC [pc]'s master volume as Android's volume controls see it, in [VOLUME_STEPS]. */
    private inner class PcVolume(val pc: String, private var level: PcLevel) :
        VolumeProvider(VOLUME_CONTROL_ABSOLUTE, VOLUME_STEPS, toSteps(level.volume)) {

        fun show(level: PcLevel) {
            this.level = level
            currentVolume = toSteps(level.volume)
        }

        override fun onSetVolumeTo(volume: Int) {
            main.post { set(volume.coerceIn(0, VOLUME_STEPS), null) }
        }

        override fun onAdjustVolume(direction: Int) {
            main.post {
                when (direction) {
                    AudioManager.ADJUST_TOGGLE_MUTE -> set(null, !level.muted)
                    AudioManager.ADJUST_MUTE -> set(null, true)
                    AudioManager.ADJUST_UNMUTE -> set(null, false)
                    AudioManager.ADJUST_SAME -> {}
                    // Turning it up unmutes it, as the PC's own keys do.
                    else -> set(
                        (currentVolume + direction.coerceIn(-1, 1)).coerceIn(0, VOLUME_STEPS),
                        if (direction > 0 && level.muted) false else null,
                    )
                }
            }
        }

        private fun set(steps: Int?, muted: Boolean?) {
            if (remote !== this) return
            val volume = steps?.let { it * 100 / VOLUME_STEPS }
            show(PcLevel(volume ?: level.volume, muted ?: level.muted))
            setPcAudio(pc, volume, muted)
        }
    }

    private fun hide() {
        main.removeCallbacks(stalePauseTimeout)
        main.removeCallbacks(undoPressedPlaying)
        shown = null
        remote = null
        if (activeSeedColor != null) {
            activeSeedColor = null
            onSeedColorChanged()
        }
        session?.run {
            isActive = false
            release()
        }
        session = null
        context.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_TAG, NOTIFICATION_ID)
    }

    private fun notification(
        session: MediaSession,
        player: MediaPlayer,
        cleanTitle: String,
        subtitleLine: String,
        picture: Bitmap?,
    ): Notification {
        val buttons = buildList {
            if (MediaAction.PREVIOUS in player.actions) add(button(R.drawable.ic_skip_previous, R.string.media_previous, MediaAction.PREVIOUS))
            if (player.playing && MediaAction.PAUSE in player.actions) add(button(R.drawable.ic_pause, R.string.media_pause, MediaAction.PAUSE))
            if (!player.playing && MediaAction.PLAY in player.actions) add(button(R.drawable.ic_play, R.string.media_play, MediaAction.PLAY))
            if (MediaAction.NEXT in player.actions) add(button(R.drawable.ic_skip_next, R.string.media_next, MediaAction.NEXT))
        }
        return Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(cleanTitle)
            .setContentText(subtitleLine)
            .setLargeIcon(picture)
            .setContentIntent(openApp())
            .setOngoing(player.playing)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setCategory(Notification.CATEGORY_TRANSPORT)
            .apply { buttons.forEach(::addAction) }
            .setStyle(
                Notification.MediaStyle()
                    .setMediaSession(session.sessionToken)
                    .setShowActionsInCompactView(*buttons.indices.take(3).toIntArray()),
            )
            .build()
    }

    private fun button(icon: Int, label: Int, action: MediaAction): Notification.Action {
        val intent = Intent(context, CommandReceiver::class.java).putExtra(EXTRA_ACTION, action.name)
        val pending = PendingIntent.getBroadcast(
            context,
            action.ordinal,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Action.Builder(Icon.createWithResource(context, icon), context.getString(label), pending).build()
    }

    private fun openApp(): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_IMMUTABLE,
    )

    private fun post(notification: Notification) {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            return
        }
        context.getSystemService(NotificationManager::class.java).notify(NOTIFICATION_TAG, NOTIFICATION_ID, notification)
    }

    /** The notification's buttons on Android 12 and earlier (later ones use the session), plus widget controls. */
    class CommandReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val action = intent.getStringExtra(EXTRA_ACTION)?.let { name -> MediaAction.entries.find { it.name == name } } ?: return
            (context.applicationContext as G7LinkApplication).core.pcMedia.pressed(action)
        }
    }

    /** A button in the notification or widget: the PC playing there. */
    internal fun pressed(action: MediaAction) = main.post {
        val (pc, player) = shown ?: return@post
        press(pc, player.id, action)
    }

    /**
     * A button in the app for [pc]'s foremost player. Play and pause show at
     * once; the PC's next update confirms it, or it goes back after [PRESSED_PLAYING_MS].
     */
    fun pressed(pc: String, action: MediaAction) = main.post {
        val summary = _summaries.value[pc] ?: return@post
        press(pc, summary.playerId, action)
    }

    /** A button in the app's player for [pc]'s [player], as [pressed]. */
    fun pressed(pc: String, player: String, action: MediaAction) = main.post { press(pc, player, action) }

    private fun press(pc: String, player: String, action: MediaAction) {
        send(pc, player, action, null)
        val playing = when (action) {
            MediaAction.PLAY -> true
            MediaAction.PAUSE -> false
            else -> return
        }
        val foremost = _summaries.value[pc]?.takeIf { it.playerId == player && it.playing != playing }
        val listed = _everyPlayer.value.any { it.pc == pc && it.playerId == player && it.playing != playing }
        if (foremost == null && !listed) return
        if (foremost != null) _summaries.value = _summaries.value + (pc to foremost.copy(playing = playing))
        if (listed) {
            _everyPlayer.value = _everyPlayer.value.map {
                if (it.pc == pc && it.playerId == player) it.copy(playing = playing) else it
            }
        }
        main.removeCallbacks(undoPressedPlaying)
        main.postDelayed(undoPressedPlaying, PRESSED_PLAYING_MS)
        if (foremost != null) app.g7link.android.widget.PcWidgetProvider.refreshAll(context)
    }

    companion object {
        private const val TAG = "PcMedia"
        private const val CHANNEL = "pc_media"
        private const val NOTIFICATION_TAG = "pc_media"
        private const val NOTIFICATION_ID = 1
        const val EXTRA_ACTION = "action"

        /** How long a pressed play or pause shows without the PC confirming it. */
        private const val PRESSED_PLAYING_MS = 4_000L

        /** Steps of the PC's volume on the phone: each button press moves it 4%. */
        internal const val VOLUME_STEPS = 25

        /** A PC volume (`0..=100`) as the nearest step. */
        internal fun toSteps(volume: Int): Int = ((volume.coerceIn(0, 100) * VOLUME_STEPS + 50) / 100)

        /** Normal speed, in the thousandths [Timeline.rateMilli] counts. */
        const val NORMAL_RATE = 1000

        /** The rates a PC may report (0.25× to 4×); anything else counts as normal speed. */
        private const val MIN_RATE = 250L
        private const val MAX_RATE = 4000L

        /** The oldest a position may say it is; older ones count as this old (the duration caps it anyway). */
        internal const val MAX_AGE_MS = 60_000L

        /** How long a seek from here outranks the PC's positions. */
        internal const val SEEK_HOLD_MS = 1_000L

        /** How close the PC's position must be to a seek to confirm it. */
        internal const val POSITION_SLACK_MS = 1_500L

        internal const val SHUFFLE_ON = "on"
        internal const val SHUFFLE_OFF = "off"

        /** The wire's `loop` value for [repeat]. */
        internal fun repeatValue(repeat: MediaRepeat): String = when (repeat) {
            MediaRepeat.NONE -> "none"
            MediaRepeat.TRACK -> "track"
            MediaRepeat.PLAYLIST -> "playlist"
        }

        /** [player]'s rate in thousandths: normal speed when it gives none or one out of range. */
        internal fun rateOf(player: MediaPlayer): Int =
            player.rateMilli?.toLong()?.takeIf { it in MIN_RATE..MAX_RATE }?.toInt() ?: NORMAL_RATE

        /** How old [player]'s position was when the PC sent it, at most [MAX_AGE_MS]. */
        internal fun ageOf(player: MediaPlayer): Long =
            player.ageMs?.let { if (it > MAX_AGE_MS.toULong()) MAX_AGE_MS else it.toLong() } ?: 0L

        /**
         * [player]'s timeline as it arrived at [arrivedAt]: its position was sampled its age
         * before that, and moves on at its rate. Null without a duration.
         */
        internal fun timelineOf(player: MediaPlayer, arrivedAt: Long): Timeline? {
            val duration = player.duration?.toLong()?.takeIf { it > 0 } ?: return null
            return Timeline(
                positionMs = (player.position?.toLong() ?: 0L).coerceIn(0, duration),
                durationMs = duration,
                playing = player.playing,
                canSeek = MediaAction.SEEK in player.actions,
                atElapsedMs = arrivedAt - ageOf(player),
                rateMilli = rateOf(player),
            )
        }

        /**
         * What to show when [incoming] arrives at [now] while [hold] is in force, and whether the
         * hold stays: the seek's position (moving on as [incoming] says it plays) until the PC's
         * position is within [POSITION_SLACK_MS] of it or the hold is [SEEK_HOLD_MS] old.
         */
        internal fun settled(incoming: Timeline, hold: SeekHold, now: Long): Pair<Timeline, Boolean> {
            if (now - hold.at >= SEEK_HOLD_MS) return incoming to false
            val target = hold.shown.positionAt(now)
            if (kotlin.math.abs(incoming.positionAt(now) - target) <= POSITION_SLACK_MS) return incoming to false
            return incoming.copy(positionMs = target.coerceIn(0, incoming.durationMs), atElapsedMs = now) to true
        }

        /** Artwork is decoded to about this many pixels on its longer side, plenty for a notification. */
        private const val ART_PX = 512

        /** The power-of-two sample size that brings a `width` x `height` image down to about [ART_PX]. */
        internal fun artSampleSize(width: Int, height: Int): Int {
            var sample = 1
            while (maxOf(width, height) / (sample * 2) >= ART_PX) sample *= 2
            return sample
        }

        /**
         * Decodes the PC's artwork small, whatever size it claims: a small file can
         * describe a huge image, which decoded in full would run the app out of memory.
         */
        private fun decodeArtwork(bytes: ByteArray): Bitmap? {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
            val options = BitmapFactory.Options().apply { inSampleSize = artSampleSize(bounds.outWidth, bounds.outHeight) }
            return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        }

        private fun isOpaqueId(s: String): Boolean {
            val trimmed = s.trim()
            if (trimmed.isEmpty()) return true
            if (trimmed.startsWith("_crx_", ignoreCase = true)) return true
            return trimmed.length >= 8 &&
                trimmed.any { it in '0'..'9' } &&
                trimmed.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' || it == '-' || it == '_' }
        }

        /**
         * Strips paths, hex hashes and GUIDs from a PC player's application identifier
         * so Android notifications never show random hex strings.
         */
        internal fun sanitizeAppName(raw: String?): String {
            val trimmed = raw?.trim().orEmpty()
            if (trimmed.isEmpty()) return ""
            val head = trimmed.substringAfterLast('/').trim()
            if (head.isEmpty()) return ""
            val lower = head.lowercase()
            if (lower.contains("firefox")) return "Firefox"
            if (isOpaqueId(head)) return ""
            return when {
                lower.contains("chrome") -> "Chrome"
                lower.contains("msedge") || lower == "edge" -> "Edge"
                lower.contains("brave") -> "Brave"
                lower.contains("vivaldi") -> "Vivaldi"
                lower.contains("opera") -> "Opera"
                lower.contains("spotify") -> "Spotify"
                else -> head
            }
        }

        /**
         * Strips raw hex hashes or GUIDs that sometimes leak into `artist`
         * without corrupting legitimate artist names like `"AC/DC"`, `"Panic! At The Disco"`, or `"Chromeo"`.
         */
        internal fun sanitizeArtistLabel(raw: String?): String {
            val trimmed = raw?.trim().orEmpty()
            if (trimmed.isEmpty()) return ""
            if (isOpaqueId(trimmed)) return ""
            return trimmed
        }

        fun createChannel(context: Context) {
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL, context.getString(R.string.channel_pc_media), NotificationManager.IMPORTANCE_LOW)
                    .apply { setShowBadge(false) },
            )
        }
    }
}
