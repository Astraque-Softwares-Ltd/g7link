// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.audiofx.LoudnessEnhancer
import android.os.Build
import android.os.Process
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Plays live PCM-16LE sound streamed from a paired PC (docs/protocol/v1.md §5)
 * on a low-latency output (`PERFORMANCE_MODE_LOW_LATENCY`), while a
 * dedicated `THREAD_PRIORITY_URGENT_AUDIO` worker keeps a tight jitter
 * cushion ([SoundJitterBuffer]) without ever calling `pause()`/`flush()`
 * mid-stream. Pieces play in order; one older than the last played is dropped.
 */
class PcSpeakerPlayer(
    private val activity: ActivityWindow = ActivityWindow(ACTIVE_WINDOW_MS),
) {
    private val buffer = SoundJitterBuffer()
    private val workerRunning = AtomicBoolean(false)
    private val watchdogRunning = AtomicBoolean(false)
    @Volatile
    private var watchdogThread: Thread? = null

    private val _playing = MutableStateFlow(false)

    /** True while audio frames arrived within the trailing [ACTIVE_WINDOW_MS]. */
    val playing: StateFlow<Boolean> = _playing.asStateFlow()

    @Volatile
    var activePcId: String? = null
        private set

    /**
     * Queues a piece of PC sound: [seq] counts up by one per piece and wraps at 2^16, [tsMs] is
     * when its first sample was captured on the PC.
     */
    fun playFrame(sampleRate: Int, channels: Int, seq: Int, tsMs: Long, pcm: ByteArray, pcId: String? = null) {
        if (pcm.isEmpty() || sampleRate <= 0 || channels <= 0) return
        if (pcId != null) {
            // Another PC's sound is numbered on its own.
            if (activePcId != null && activePcId != pcId) buffer.restart()
            activePcId = pcId
        }
        activity.markActive()
        if (!_playing.value) {
            _playing.value = true
        }
        ensureWatchdog()

        val safeRate = sampleRate.coerceIn(8_000, 96_000)
        val safeChannels = if (channels >= 2) 2 else 1
        // The buffer caps its backlog at ~40 ms so Wi-Fi bursts never build up
        // latency and we never need to flush the hardware AudioTrack.
        buffer.offer(SoundPiece(seq and 0xFFFF, tsMs, safeRate, safeChannels, pcm))

        if (workerRunning.compareAndSet(false, true)) {
            Thread(::runAudioLoop, "g7link-pc-speaker").apply {
                isDaemon = true
                start()
            }
        }
    }

    fun stopForPc(pcId: String) {
        if (activePcId == pcId) {
            stop()
        }
    }

    fun stop() {
        activePcId = null
        activity.reset()
        _playing.value = false
        watchdogThread?.interrupt()
        // A waiting worker stops at once.
        buffer.stop()
    }

    private fun ensureWatchdog() {
        if (watchdogRunning.compareAndSet(false, true)) {
            Thread(::runWatchdogLoop, "g7link-pc-speaker-watchdog").apply {
                isDaemon = true
                start()
            }
        }
    }

    private fun runWatchdogLoop() {
        watchdogThread = Thread.currentThread()
        try {
            while (true) {
                Thread.sleep(WATCHDOG_CHECK_INTERVAL_MS)
                if (!activity.isActive()) {
                    _playing.value = false
                    break
                }
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            _playing.value = false
        } finally {
            watchdogThread = null
            watchdogRunning.set(false)
            if (activity.isActive() && watchdogRunning.compareAndSet(false, true)) {
                _playing.value = true
                Thread(::runWatchdogLoop, "g7link-pc-speaker-watchdog").apply {
                    isDaemon = true
                    start()
                }
            }
        }
    }

    private fun runAudioLoop() {
        runCatching {
            Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        }

        var track: AudioTrack? = null
        var enhancer: LoudnessEnhancer? = null
        var currentRate = 0
        var currentChannels = 0
        var needsPrebuffer = true

        try {
            while (true) {
                val next = buffer.poll(INACTIVITY_TIMEOUT_MS) ?: break
                val first = next.piece

                if (track == null ||
                    track.state != AudioTrack.STATE_INITIALIZED ||
                    currentRate != first.sampleRate ||
                    currentChannels != first.channels
                ) {
                    releaseTrack(track, enhancer)
                    track = createTrack(first.sampleRate, first.channels)
                    enhancer = track?.let { createEnhancer(it.audioSessionId) }
                    currentRate = first.sampleRate
                    currentChannels = first.channels
                    needsPrebuffer = true
                }

                val activeTrack = track ?: continue

                if (needsPrebuffer || next.fresh) {
                    // On a start, or after the sound stopped for a while, gather a cushion
                    // (10–30 ms, twice the measured jitter) so Wi-Fi jitter doesn't cause an
                    // underrun at once, while keeping latency minimal.
                    val cushionUs = buffer.cushionUs()
                    val maxWaitMs = cushionUs / 1_000L
                    var waitedMs = 0L
                    while (buffer.bufferedUs() + first.durationUs < cushionUs && waitedMs < maxWaitMs) {
                        Thread.sleep(2)
                        waitedMs += 2
                    }
                    needsPrebuffer = false
                }

                if (activeTrack.playState != AudioTrack.PLAYSTATE_PLAYING) {
                    runCatching { activeTrack.play() }
                }

                val startedWriteNs = System.nanoTime()
                runCatching {
                    activeTrack.write(first.pcm, 0, first.pcm.size, AudioTrack.WRITE_BLOCKING)
                }.onFailure {
                    Log.w(TAG, "AudioTrack write failed", it)
                }
                val writeMs = (System.nanoTime() - startedWriteNs) / 1_000_000L
                if (writeMs > 25L) {
                    Log.d(TAG, "AudioTrack write blocked ${writeMs}ms (buffered=${buffer.bufferedUs() / 1_000L}ms)")
                }
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        } finally {
            workerRunning.set(false)
            releaseTrack(track, enhancer)
            if (buffer.late > 0L || buffer.lost > 0L) {
                Log.d(TAG, "PC sound: ${buffer.late} late, ${buffer.lost} lost")
            }
            // If new frames arrived just as the loop exited, restart the worker.
            val hasPending = !buffer.isEmpty()
            if (hasPending && workerRunning.compareAndSet(false, true)) {
                Thread(::runAudioLoop, "g7link-pc-speaker").apply {
                    isDaemon = true
                    start()
                }
            }
        }
    }

    private fun createTrack(sampleRate: Int, channels: Int): AudioTrack? {
        val channelMask = if (channels == 1) {
            AudioFormat.CHANNEL_OUT_MONO
        } else {
            AudioFormat.CHANNEL_OUT_STEREO
        }
        val minBuf = AudioTrack.getMinBufferSize(
            sampleRate,
            channelMask,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minBuf <= 0) {
            Log.w(TAG, "invalid minBufferSize=$minBuf for rate=$sampleRate channels=$channels")
            return null
        }
        val frameBytes = channels * 2
        // ~24 ms hardware buffer floor balances low latency with Wi-Fi jitter resilience.
        val targetBuf = minBuf.coerceAtLeast((sampleRate * frameBytes * 24) / 1000)

        return runCatching {
            val attrs = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .apply {
                    // The PC's own sound must not go back to the PC with the
                    // phone's sound or a mirror, or it plays twice and can loop.
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                        setAllowedCapturePolicy(AudioAttributes.ALLOW_CAPTURE_BY_SYSTEM)
                    }
                }
                .build()
            val format = AudioFormat.Builder()
                .setSampleRate(sampleRate)
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setChannelMask(channelMask)
                .build()
            AudioTrack.Builder()
                .setAudioAttributes(attrs)
                .setAudioFormat(format)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setBufferSizeInBytes(targetBuf)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .build()
        }.getOrElse {
            Log.w(TAG, "failed to create AudioTrack", it)
            null
        }
    }

    private fun createEnhancer(audioSessionId: Int): LoudnessEnhancer? {
        if (audioSessionId == AudioTrack.ERROR_BAD_VALUE || audioSessionId == 0) return null
        return runCatching {
            LoudnessEnhancer(audioSessionId).apply {
                // Subtle +1.5 dB (150 mB) presence lift for phone speakers.
                setTargetGain(150)
                enabled = true
            }
        }.getOrNull()
    }

    private fun releaseTrack(track: AudioTrack?, enhancer: LoudnessEnhancer?) {
        runCatching { enhancer?.release() }
        if (track == null) return
        runCatching {
            if (track.playState == AudioTrack.PLAYSTATE_PLAYING) {
                track.stop()
            }
        }
        runCatching { track.release() }
    }

    companion object {
        private const val TAG = "PcSpeakerPlayer"
        private const val INACTIVITY_TIMEOUT_MS = 30_000L
        const val ACTIVE_WINDOW_MS = 3_000L
        private const val WATCHDOG_CHECK_INTERVAL_MS = 1_000L
    }
}
