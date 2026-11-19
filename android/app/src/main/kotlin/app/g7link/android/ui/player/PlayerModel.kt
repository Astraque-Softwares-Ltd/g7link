// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import app.g7link.android.media.PcMedia
import app.g7link.core.FfiRolePalette
import app.g7link.core.FfiThemeScheme
import app.g7link.core.MediaRepeat
import java.util.Locale

/** The player's decisions, kept out of the composables so JVM tests can check them. */
object PlayerModel {
    /** The player someone chose in the sheet, and when (`elapsedRealtime`). */
    data class Pick(val pc: String, val player: String, val at: Long)

    /**
     * The player the mini and full player show: the one [pick] chose while it's there and nothing
     * has started playing since; else the one that most recently started playing; else the latest
     * to appear or change. Ties go to the PCs' order. Null when nothing is there.
     */
    /** How long a player may vanish (a track change) before the players close. */
    const val GONE_GRACE_MS = 3_000L

    fun shown(players: List<PcMedia.Summary>, pick: Pick?): PcMedia.Summary? {
        if (players.isEmpty()) return null
        if (pick != null) {
            val picked = players.firstOrNull { it.pc == pick.pc && it.playerId == pick.player }
            val overtaken = players.any { it.playing && it.activeAt > pick.at && !(it.pc == pick.pc && it.playerId == pick.player) }
            if (picked != null && !overtaken) return picked
        }
        return players.filter { it.playing }.maxByOrNull { it.activeAt } ?: players.maxByOrNull { it.activeAt }
    }

    /** Whether the full player's header opens the sheet: there's another player or an output to choose. */
    fun hasChoices(players: List<PcMedia.Summary>, outputs: Int, phoneSpeaker: Boolean): Boolean =
        players.size > 1 || outputs > 0 || phoneSpeaker

    /** Which of a player's controls work. Shuffle and repeat aren't shown at all when they don't. */
    data class Controls(
        val playPause: Boolean,
        val previous: Boolean,
        val next: Boolean,
        val seek: Boolean,
        val shuffle: Boolean,
        val repeat: Boolean,
    )

    fun controls(track: PcMedia.Summary, timeline: PcMedia.Timeline?): Controls = Controls(
        playPause = if (track.playing) track.canPause else track.canPlay,
        previous = track.canPrevious,
        next = track.canNext,
        seek = track.canSeek && timeline != null && timeline.canSeek && timeline.durationMs > 0,
        shuffle = track.canShuffle,
        repeat = track.canRepeat,
    )

    /** What the repeat button sets next: off, the playlist, this track, off again. */
    fun nextRepeat(current: MediaRepeat?): MediaRepeat = when (current) {
        null, MediaRepeat.NONE -> MediaRepeat.PLAYLIST
        MediaRepeat.PLAYLIST -> MediaRepeat.TRACK
        MediaRepeat.TRACK -> MediaRepeat.NONE
    }

    /** How much of [timeline] has played at [now], 0 to 1. */
    fun progress(timeline: PcMedia.Timeline?, now: Long): Float {
        if (timeline == null || timeline.durationMs <= 0) return 0f
        return (timeline.positionAt(now).toFloat() / timeline.durationMs).coerceIn(0f, 1f)
    }

    /** A time as the seek bar shows it: "1:04", or "1:02:03" past an hour. */
    fun clock(ms: Long): String {
        val total = ms.coerceAtLeast(0) / 1000
        val hours = total / 3600
        val minutes = total % 3600 / 60
        val seconds = total % 60
        return if (hours > 0) {
            String.format(Locale.ROOT, "%d:%02d:%02d", hours, minutes, seconds)
        } else {
            String.format(Locale.ROOT, "%d:%02d", minutes, seconds)
        }
    }

    /** The volume step (of [PcMedia.VOLUME_STEPS], 4 % each) nearest [fraction] of the bar. */
    fun volumeStep(fraction: Float): Int =
        (fraction.coerceIn(0f, 1f) * PcMedia.VOLUME_STEPS + 0.5f).toInt().coerceIn(0, PcMedia.VOLUME_STEPS)

    /** A volume step as the PC's volume, `0..=100`. */
    fun volumeOf(step: Int): Int = step.coerceIn(0, PcMedia.VOLUME_STEPS) * 100 / PcMedia.VOLUME_STEPS

    /** Which way a sideways swipe of [dx] skips: 1 next (to the left), -1 previous, 0 neither. */
    fun skip(dx: Float, threshold: Float): Int = when {
        dx <= -threshold -> 1
        dx >= threshold -> -1
        else -> 0
    }

    /** How far a drag must take the player open or closed for it to settle there on release. */
    const val SETTLE_FRACTION = 0.25f

    /**
     * The extractor's roles the player paints with: the dark ones in the dark theme, the light
     * ones in the light; none (the app's own) without a cover or for a black-and-white one.
     */
    fun roles(scheme: FfiThemeScheme?, dark: Boolean): FfiRolePalette? = when {
        scheme == null || scheme.achromatic -> null
        dark -> scheme.dark
        else -> scheme.light
    }
}
