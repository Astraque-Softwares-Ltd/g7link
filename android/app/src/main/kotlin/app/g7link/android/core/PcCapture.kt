// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import android.content.Context
import android.os.SystemClock
import android.util.Log
import app.g7link.android.R
import app.g7link.core.G7LinkNode
import app.g7link.core.PcCaptureOutcome
import app.g7link.core.Transfer
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What's under way with one PC's screen, asked from this phone. */
data class PcCaptureState(
    /** A screenshot asked for and not here yet. */
    val shooting: Boolean = false,
    /** Starting or stopping a recording: waiting for the PC. */
    val recordingBusy: Boolean = false,
    /** When the recording started ([SystemClock.elapsedRealtime] ms), while it records. */
    val recordingSince: Long? = null,
    /** The PC shows its screen-sharing dialog: someone has to approve it there. */
    val approve: Boolean = false,
)

/** A screenshot of a PC, ready to crop: a PNG in the app's cache. */
data class PcShot(val pcId: String, val pcName: String, val file: File)

/**
 * Screenshots and recordings of a PC's screen (docs/protocol/pc-capture.md):
 * asks the PC, says what came of it, and catches the screenshot's transfer
 * so it opens to crop instead of landing in Downloads. Recordings arrive as
 * any other file.
 */
class PcCapture internal constructor(
    private val context: Context,
    private val scope: CoroutineScope,
    private val node: suspend () -> G7LinkNode?,
    private val nameOf: (String) -> String,
    private val say: (String) -> Unit,
) {
    private val _states = MutableStateFlow<Map<String, PcCaptureState>>(emptyMap())
    /** By PC ID; PCs with nothing under way are left out. */
    val states: StateFlow<Map<String, PcCaptureState>> = _states.asStateFlow()

    private val _shot = MutableStateFlow<PcShot?>(null)
    /** The newest screenshot waiting to be cropped, until [shotShown]. */
    val shot: StateFlow<PcShot?> = _shot.asStateFlow()

    /** Screenshots on their way: the transfer bringing each, to its PC. */
    private val expected = ConcurrentHashMap<String, String>()
    /** Transfers taken as screenshots before the PC's answer named them. */
    private val claimed = ConcurrentHashMap.newKeySet<String>()

    private fun update(pcId: String, change: (PcCaptureState) -> PcCaptureState) {
        _states.update { all ->
            val next = change(all[pcId] ?: PcCaptureState())
            if (next == PcCaptureState()) all - pcId else all + (pcId to next)
        }
    }

    /** Asks the PC for a screenshot; it opens to crop once it's here. */
    fun screenshot(pcId: String) {
        if (_states.value[pcId]?.shooting == true) return
        update(pcId) { it.copy(shooting = true, approve = false) }
        scope.launch {
            val outcome = ask { it.pcCaptureShot(pcId) }
            update(pcId) { it.copy(approve = false) }
            if (outcome is PcCaptureOutcome.File) {
                if (!claimed.remove(outcome.transfer)) {
                    expected[outcome.transfer] = pcId
                    giveUpLater(outcome.transfer, pcId)
                }
            } else {
                update(pcId) { it.copy(shooting = false) }
                say(describe(pcId, outcome))
            }
        }
    }

    /** Starts recording the PC's screen, or stops it (the video then comes as a file). */
    fun toggleRecording(pcId: String) {
        val state = _states.value[pcId] ?: PcCaptureState()
        if (state.recordingBusy) return
        update(pcId) { it.copy(recordingBusy = true, approve = false) }
        if (state.recordingSince == null) {
            scope.launch {
                val outcome = ask { it.pcCaptureRecordStart(pcId) }
                val since = SystemClock.elapsedRealtime().takeIf { outcome is PcCaptureOutcome.Started }
                update(pcId) { it.copy(recordingBusy = false, approve = false, recordingSince = since) }
                if (since == null) say(describe(pcId, outcome))
            }
        } else {
            scope.launch {
                val outcome = ask { it.pcCaptureRecordStop(pcId) }
                update(pcId) { it.copy(recordingBusy = false, recordingSince = null) }
                when (outcome) {
                    is PcCaptureOutcome.File -> say(context.getString(R.string.pc_capture_recording_sent, nameOf(pcId)))
                    // It ended on the PC already, which said so.
                    is PcCaptureOutcome.NotRecording -> {}
                    else -> say(describe(pcId, outcome))
                }
            }
        }
    }

    private suspend fun ask(call: suspend (G7LinkNode) -> PcCaptureOutcome): PcCaptureOutcome? =
        try {
            node()?.let { call(it) }
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "capture request failed", e)
            null
        }

    /** A screenshot that never arrives stops being waited for. */
    private fun giveUpLater(transferId: String, pcId: String) {
        scope.launch {
            delay(SHOT_ARRIVAL_MS)
            if (expected.remove(transferId) != null) update(pcId) { it.copy(shooting = false) }
        }
    }

    private fun describe(pcId: String, outcome: PcCaptureOutcome?): String {
        val pc = nameOf(pcId)
        return when (outcome) {
            is PcCaptureOutcome.Off -> context.getString(R.string.pc_capture_off, pc)
            is PcCaptureOutcome.Locked -> context.getString(R.string.pc_capture_locked, pc)
            is PcCaptureOutcome.Declined -> context.getString(R.string.pc_capture_declined, pc)
            is PcCaptureOutcome.FilesOff -> context.getString(R.string.pc_capture_files_off, pc)
            is PcCaptureOutcome.Busy -> context.getString(R.string.pc_capture_busy, pc)
            is PcCaptureOutcome.Unsupported -> context.getString(R.string.pc_capture_unsupported, pc)
            is PcCaptureOutcome.Offline, null -> context.getString(R.string.error_not_connected)
            is PcCaptureOutcome.File,
            is PcCaptureOutcome.Started,
            is PcCaptureOutcome.NotRecording,
            is PcCaptureOutcome.Failed,
            -> context.getString(R.string.pc_capture_failed, pc)
        }
    }

    /** The PC shows its screen-sharing dialog for a request of this phone's. */
    internal fun onPrompt(pcId: String) {
        val state = _states.value[pcId] ?: return
        if (state.shooting || state.recordingBusy) update(pcId) { it.copy(approve = true) }
    }

    /** The PC ended a recording this phone started; [name] is the video on its way. */
    internal fun onRecordingEnded(pcId: String, name: String?) {
        if (_states.value[pcId]?.recordingSince == null) return
        update(pcId) { it.copy(recordingSince = null, recordingBusy = false) }
        val pc = nameOf(pcId)
        say(
            if (name != null) {
                context.getString(R.string.pc_capture_recording_sent, pc)
            } else {
                context.getString(R.string.pc_capture_recording_ended, pc)
            },
        )
    }

    /** A PC that went away ended its recording; a screenshot still comes when it's back. */
    internal fun onOffline(pcId: String) {
        update(pcId) { it.copy(recordingSince = null, recordingBusy = false, approve = false) }
    }

    /**
     * Takes a received transfer that is a screenshot this phone asked for:
     * named by the PC's answer, or (when it came first) the one PNG from a PC
     * this phone waits on. Returns false for anything else.
     */
    internal fun claim(transfer: Transfer, saved: List<String>): Boolean {
        val pcId = transfer.deviceId
        val named = expected.remove(transfer.id) != null
        val unnamed = !named &&
            _states.value[pcId]?.shooting == true &&
            !expected.containsValue(pcId) &&
            transfer.files.toInt() == 1 &&
            transfer.names.singleOrNull()?.endsWith(".png", ignoreCase = true) == true
        if (!named && !unnamed) return false
        if (unnamed) claimed.add(transfer.id)
        update(pcId) { it.copy(shooting = false, approve = false) }
        val received = saved.singleOrNull()?.let(::File) ?: return true
        val pcName = nameOf(pcId)
        scope.launch(Dispatchers.IO) {
            val kept = keep(received)
            if (kept != null) {
                _shot.value = PcShot(pcId, pcName, kept)
            } else {
                say(context.getString(R.string.pc_shot_unreadable))
            }
        }
        return true
    }

    /** A screenshot's transfer failed or was cancelled. */
    internal fun onTransferGone(transferId: String) {
        val pcId = expected.remove(transferId) ?: return
        update(pcId) { it.copy(shooting = false) }
        say(context.getString(R.string.pc_capture_failed, nameOf(pcId)))
    }

    /** The crop screen has the screenshot. */
    fun shotShown(shot: PcShot) {
        _shot.compareAndSet(shot, null)
    }

    /** Moves a received screenshot into the app's cache, where only the newest few stay. */
    private fun keep(received: File): File? {
        val dir = shotsDir(context)
        if (!dir.isDirectory && !dir.mkdirs()) return null
        dir.listFiles()
            ?.filter { it.isFile }
            ?.sortedByDescending { it.lastModified() }
            ?.drop(KEEP_SHOTS - 1)
            ?.forEach { it.delete() }
        val target = File(dir, "${System.currentTimeMillis()}-${received.name.substringAfterLast('/')}")
        val moved = received.renameTo(target) || runCatching {
            received.copyTo(target, overwrite = true)
            received.delete()
        }.isSuccess
        return target.takeIf { moved && it.isFile }
    }

    companion object {
        private const val TAG = "PcCapture"
        private const val KEEP_SHOTS = 5
        /** How long a screenshot the PC is sending is waited for. */
        private const val SHOT_ARRIVAL_MS = 120_000L

        /** Where screenshots of a PC wait to be cropped (shared through the app's FileProvider). */
        fun shotsDir(context: Context): File = File(context.cacheDir, "pc-shots")
    }
}

/** Formatting for screenshots and recordings of a PC. */
object PcCaptureFormat {
    /** A recording's length: "0:42", "12:05", "1:02:03". */
    fun elapsed(ms: Long): String {
        val total = ms.coerceAtLeast(0L) / 1_000L
        val hours = total / 3_600L
        val minutes = (total % 3_600L) / 60L
        val seconds = total % 60L
        return if (hours > 0L) {
            String.format(java.util.Locale.ROOT, "%d:%02d:%02d", hours, minutes, seconds)
        } else {
            String.format(java.util.Locale.ROOT, "%d:%02d", minutes, seconds)
        }
    }
}
