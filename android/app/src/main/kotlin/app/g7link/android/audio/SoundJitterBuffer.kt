// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.audio

import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlin.math.abs

/**
 * A piece of a PC's sound (docs/protocol/v1.md §5): [seq] counts up by one per piece and wraps at
 * 2^16; [tsMs] is when its first sample was captured, in ms since the PC's sound began.
 */
class SoundPiece(
    val seq: Int,
    val tsMs: Long,
    val sampleRate: Int,
    val channels: Int,
    val pcm: ByteArray,
) {
    val durationUs: Long = (pcm.size / (channels * 2)).toLong() * 1_000_000L / sampleRate
}

/** The next piece to play; [fresh] when the track should gather its cushion before playing it. */
class SoundOut(val piece: SoundPiece, val fresh: Boolean)

/**
 * PC sound waiting to be played, in [SoundPiece.seq] order. A piece that isn't after the last one
 * played (or let go to keep the delay down) is late and dropped; one far behind it means the PC's
 * sound started again. Arrival times against capture times ([SoundPiece.tsMs]) measure the
 * network's jitter, which sets how much sound to gather before playing ([cushionUs]); a jump in
 * capture time means the sound stopped for a while, and the cushion is gathered again.
 */
class SoundJitterBuffer(
    private val maxBufferedUs: Long = MAX_BUFFERED_US,
    private val maxPieces: Int = MAX_PIECES,
    private val nowMs: () -> Long = { System.nanoTime() / 1_000_000L },
) {
    private val lock = ReentrantLock()
    private val arrived = lock.newCondition()
    private val pieces = ArrayList<SoundPiece>()

    /** The last piece played or let go: nothing at or before it plays. */
    private var last: SoundPiece? = null
    private var lastArrivalMs: Long? = null
    private var lastArrivalTsMs = 0L

    /** Interarrival jitter (RFC 3550 §6.4.1), in µs. */
    private var jitterUs = 0L
    private var stopping = false

    /** Pieces dropped as late or repeated. */
    @Volatile
    var late = 0L
        private set

    /** Pieces that never came: gaps in [SoundPiece.seq] between pieces played. */
    @Volatile
    var lost = 0L
        private set

    /** Takes a piece that just arrived; false when it's late or a repeat. */
    fun offer(piece: SoundPiece): Boolean {
        lock.withLock {
            val now = nowMs()
            lastArrivalMs?.let { if (now - it > RESTART_AFTER_IDLE_MS) forget() }
            last?.let { done ->
                if (!isAfter(piece.seq, done.seq)) {
                    if (behind(piece.seq, done.seq) <= MAX_LATE) {
                        late++
                        return false
                    }
                    // Far behind what played: the PC's sound started again from the beginning.
                    forget()
                }
            }
            var at = pieces.size
            while (at > 0 && isAfter(pieces[at - 1].seq, piece.seq)) at--
            if (at > 0 && pieces[at - 1].seq == piece.seq) {
                late++
                return false
            }
            pieces.add(at, piece)
            measure(piece, now)
            // Past the cap the oldest go, and a straggler from before them is late.
            while (pieces.size > 1 && (pieces.size > maxPieces || queuedUs() > maxBufferedUs)) {
                last = pieces.removeAt(0)
            }
            arrived.signal()
            return true
        }
    }

    /** The next piece to play, waiting up to [timeoutMs]; null when none came or after [stop]. */
    fun poll(timeoutMs: Long): SoundOut? {
        lock.withLock {
            val startedMs = nowMs()
            var waitNs = TimeUnit.MILLISECONDS.toNanos(timeoutMs)
            while (pieces.isEmpty() && !stopping && waitNs > 0L) {
                waitNs = arrived.awaitNanos(waitNs)
            }
            if (stopping) {
                stopping = false
                return null
            }
            if (pieces.isEmpty()) return null
            // Waiting this long, the track ran dry: it gathers a cushion again.
            val starved = nowMs() - startedMs > STARVED_MS
            val piece = pieces.removeAt(0)
            val before = last
            last = piece
            if (before == null) return SoundOut(piece, fresh = true)
            lost += (((piece.seq - before.seq) and SEQ_MASK) - 1).coerceAtLeast(0)
            val expectedMs = before.tsMs + before.durationUs / 1_000L
            val fresh = starved ||
                before.sampleRate != piece.sampleRate ||
                before.channels != piece.channels ||
                abs(piece.tsMs - expectedMs) > DISCONTINUITY_MS
            return SoundOut(piece, fresh)
        }
    }

    /** How much sound waits to be played, in µs. */
    fun bufferedUs(): Long = lock.withLock { queuedUs() }

    /** How much sound to gather before playing, in µs: twice the jitter, within bounds. */
    fun cushionUs(): Long = lock.withLock { (2 * jitterUs).coerceIn(MIN_CUSHION_US, MAX_CUSHION_US) }

    fun isEmpty(): Boolean = lock.withLock { pieces.isEmpty() }

    /** Starts over, for another PC's sound. */
    fun restart() {
        lock.withLock {
            forget()
            jitterUs = 0L
        }
    }

    /** Drops everything and has a waiting [poll] return null. */
    fun stop() {
        lock.withLock {
            forget()
            jitterUs = 0L
            stopping = true
            arrived.signalAll()
        }
    }

    private fun forget() {
        pieces.clear()
        last = null
        lastArrivalMs = null
    }

    private fun queuedUs(): Long = pieces.sumOf { it.durationUs }

    private fun measure(piece: SoundPiece, now: Long) {
        val previous = lastArrivalMs
        if (previous != null) {
            val deviationMs = abs((now - previous) - (piece.tsMs - lastArrivalTsMs)).coerceAtMost(MAX_DEVIATION_MS)
            jitterUs += (deviationMs * 1_000L - jitterUs) / 16
        }
        lastArrivalMs = now
        lastArrivalTsMs = piece.tsMs
    }

    companion object {
        private const val SEQ_MASK = 0xFFFF

        /** Whether [seq] comes after [last]: less than 2^15 ahead of it, counting modulo 2^16. */
        fun isAfter(seq: Int, last: Int): Boolean = ((seq - last) and SEQ_MASK) in 1 until 0x8000

        private fun behind(seq: Int, last: Int): Int = (last - seq) and SEQ_MASK

        /** About 40 ms of sound waits at most, so Wi-Fi bursts never build up delay. */
        const val MAX_BUFFERED_US = 40_000L
        const val MAX_PIECES = 95

        /** A piece this many behind the last played is from a new start, not late (~0.3 s of pieces). */
        const val MAX_LATE = 64

        /** Nothing for this long and the next piece starts afresh. */
        const val RESTART_AFTER_IDLE_MS = 1_000L

        /** A capture time this far from where the last piece ended is a new stretch of sound. */
        const val DISCONTINUITY_MS = 40L

        /** Waiting this long for a piece outlasts the track's own buffer (~24 ms). */
        const val STARVED_MS = 30L

        const val MIN_CUSHION_US = 10_000L
        const val MAX_CUSHION_US = 30_000L
        private const val MAX_DEVIATION_MS = 60L
    }
}
