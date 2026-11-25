// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.IntentCompat
import androidx.core.content.edit
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.G7LinkApplication
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.MainActivity
import app.g7link.android.ui.Preferences
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7LinkTheme
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.g7Tints
import java.io.File
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * "Ask an agent" in Android's share sheet (contract §V6): a sheet over the
 * app that shared. It picks the PC when more than one offers agents, then a
 * new thread or one of the 5 newest that take a message, and opens it with
 * the shared text in the reply box and the pictures attached. Nothing is
 * sent from here: the user reads it and presses Send (or Start).
 *
 * It reads only the URIs the sharing app granted, copies the pictures into
 * `cacheDir/agent-share`, and deletes them when the sheet closes without
 * handing them over (the screen that takes them deletes them after upload).
 */
class AgentShareActivity : ComponentActivity() {
    private var copies: List<File> = emptyList()
    private var handedOver = false

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val incoming = incoming(intent)
        if (incoming == null) {
            finish()
            return
        }
        val core = (application as G7LinkApplication).core
        val preferences = Preferences(this)
        setContent {
            val appearance by preferences.appearance.collectAsStateWithLifecycle()
            G7LinkTheme(appearance) {
                AgentShareSheet(
                    core = core,
                    incoming = incoming,
                    read = { readPictures(incoming.streams).also { copies = it.second } },
                    onPicked = ::handOver,
                    onClose = ::finish,
                )
            }
        }
    }

    override fun onDestroy() {
        // Closed without handing them over: the copies go with the sheet.
        if (isFinishing && !handedOver) copies.forEach { runCatching { it.delete() } }
        super.onDestroy()
    }

    /** The share as it came: its text, and the pictures it offers, only `content:` URIs of other apps. */
    private fun incoming(intent: Intent?): Incoming? {
        val action = intent?.action ?: return null
        val multiple = when (action) {
            Intent.ACTION_SEND -> false
            Intent.ACTION_SEND_MULTIPLE -> true
            else -> return null
        }
        if (!AgentShareRules.takes(intent.type, multiple)) return null
        val text = if (multiple) {
            null
        } else {
            AgentShareRules.text(intent.getCharSequenceExtra(Intent.EXTRA_SUBJECT), intent.getCharSequenceExtra(Intent.EXTRA_TEXT))
        }
        val streams = if (multiple) {
            IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
        } else {
            listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
        }
        val granted = streams.filter(::shared).take(MAX_STREAMS)
        if (text == null && granted.isEmpty()) return null
        return Incoming(text, granted)
    }

    /** Another app's content, never a file path or this app's own provider (it could name this app's files). */
    private fun shared(uri: Uri): Boolean =
        uri.scheme == "content" && uri.authority?.let { it == packageName || it.startsWith("$packageName.") } == false

    /** Reads and copies the pictures, in order, until 4 are in; what doesn't go is refused by name. */
    private suspend fun readPictures(streams: List<Uri>): Pair<SharePictures, List<File>> = withContext(Dispatchers.IO) {
        val dir = File(cacheDir, AgentShareTargets.DIR)
        runCatching { if (!dir.isDirectory) dir.mkdirs() }
        val read = ArrayList<SharedPicture>()
        val files = ArrayList<File?>()
        for (uri in streams) {
            val name = displayName(uri)
            // Past the 4 a message carries, nothing more is read.
            if (files.count { it != null } >= AgentImages.MAX_PER_MESSAGE) {
                read += SharedPicture(name, null)
                files += null
                continue
            }
            val (problem, file) = copy(uri, dir)
            read += SharedPicture(name, problem)
            files += file
        }
        val pictures = AgentShareRules.pictures(read)
        val kept = pictures.added.mapNotNull { files.getOrNull(it) }
        files.filterNotNull().filter { it !in kept }.forEach { runCatching { it.delete() } }
        pictures to kept
    }

    /** One picture copied into the cache, at most 10 MiB, checked by its first bytes. */
    private fun copy(uri: Uri, dir: File): Pair<ImageProblem?, File?> {
        val file = File(dir, "share-${UUID.randomUUID()}")
        return runCatching {
            val head = java.io.ByteArrayOutputStream(HEAD_BYTES)
            val total = contentResolver.openInputStream(uri)?.use { input ->
                file.outputStream().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val n = input.read(buffer)
                        if (n < 0) break
                        if (head.size() < HEAD_BYTES) head.write(buffer, 0, minOf(n, HEAD_BYTES - head.size()))
                        total += n
                        // One byte past the limit is enough to know it's too large.
                        if (total > AgentImages.MAX_BYTES) break
                        output.write(buffer, 0, n)
                    }
                    total
                }
            } ?: return@runCatching ImageProblem.FAILED to null
            val problem = when {
                total > AgentImages.MAX_BYTES -> ImageProblem.TOO_LARGE
                AgentImages.kind(head.toByteArray()) == null -> ImageProblem.NOT_AN_IMAGE
                else -> null
            }
            if (problem != null) {
                file.delete()
                problem to null
            } else {
                null to file
            }
        }.getOrElse {
            Log.w(TAG, "can't read a shared picture", it)
            runCatching { file.delete() }
            ImageProblem.FAILED to null
        }
    }

    private fun displayName(uri: Uri): String {
        val name = runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0) else null
            }
        }.getOrNull()
        return name.orEmpty().substringAfterLast('/').filter { !it.isISOControl() }.take(MAX_NAME_CHARS).trim()
            .ifEmpty { getString(R.string.agents_share_unnamed) }
    }

    /** Opens the thread (or a new one) with the share in its box; the user sends it from there. */
    private fun handOver(pcId: String, threadId: String?, online: Boolean, text: SharedDraft?) {
        val images = if (online) copies else emptyList()
        if (!online) copies.forEach { runCatching { it.delete() } }
        AgentShared.put(pcId, threadId, SharedDraft(text?.text.orEmpty(), text?.shortened == true, images.map { it.absolutePath }))
        handedOver = true
        val open = Intent(this, MainActivity::class.java)
            .putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, pcId)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (threadId != null) open.putExtra(MainActivity.EXTRA_OPEN_AGENT_THREAD, threadId)
        runCatching { startActivity(open) }
        finish()
    }

    private companion object {
        const val TAG = "AgentShare"
        const val MAX_STREAMS = 16
        const val MAX_NAME_CHARS = 128
        const val HEAD_BYTES = 16
    }
}

/** What a share offers: its text, and the URIs of its pictures. */
internal data class Incoming(val text: SharedDraft?, val streams: List<Uri>)

/**
 * The share target's place in Android's share sheet: there only while a
 * paired PC offers agents and has them on for this phone (contract §V6), and
 * the copies it leaves behind.
 */
object AgentShareTargets {
    private const val PREFS = "agent_share"
    private const val PCS = "pcs"
    const val DIR = "agent-share"

    /** Copies older than this belong to no share in progress. */
    private const val SWEEP_AFTER_MS = 60L * 60 * 1000

    /** The PCs that offer agents to this phone, as last heard. */
    fun pcs(context: Context): Set<String> = prefs(context).getStringSet(PCS, null).orEmpty().toSet()

    /** [pcId] offers agents to this phone ([on]) or doesn't any more; the share target follows. */
    @Synchronized
    fun offer(context: Context, pcId: String, on: Boolean) {
        val before = pcs(context)
        val after = if (on) before + pcId else before - pcId
        if (after != before) prefs(context).edit { putStringSet(PCS, after) }
        val want = if (after.isEmpty()) PackageManager.COMPONENT_ENABLED_STATE_DISABLED else PackageManager.COMPONENT_ENABLED_STATE_ENABLED
        val component = ComponentName(context, AgentShareActivity::class.java)
        runCatching {
            val manager = context.packageManager
            if (manager.getComponentEnabledSetting(component) != want) {
                manager.setComponentEnabledSetting(component, want, PackageManager.DONT_KILL_APP)
            }
        }
    }

    /** Pictures shared before and never taken (the app stopped in between). */
    fun sweep(context: Context) {
        val now = System.currentTimeMillis()
        runCatching {
            File(context.cacheDir, DIR).listFiles()?.filter { now - it.lastModified() > SWEEP_AFTER_MS }?.forEach { it.delete() }
        }
    }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}

/** The sheet: the PC (when there's a choice), then where the share goes [P04b]. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AgentShareSheet(
    core: Core,
    incoming: Incoming,
    read: suspend () -> Pair<SharePictures, List<File>>,
    onPicked: (pcId: String, threadId: String?, online: Boolean, text: SharedDraft?) -> Unit,
    onClose: () -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val state by core.state.collectAsStateWithLifecycle()
    val all by core.agents.collectAsStateWithLifecycle()
    val offering = remember { AgentShareTargets.pcs(context) }
    val pcs = state.devices.filter { it.id in offering }
    var chosen by rememberSaveable { mutableStateOf<String?>(null) }
    val pc = (chosen?.let { id -> pcs.firstOrNull { it.id == id } }) ?: pcs.singleOrNull()
    var pictures by remember { mutableStateOf<SharePictures?>(null) }
    LaunchedEffect(Unit) { pictures = read().first }
    // The PC's threads: from the PC while it's reachable, else what this phone saved.
    LaunchedEffect(pc?.id, pc?.online) {
        val device = pc ?: return@LaunchedEffect
        if (device.online) core.agentThreads(device.id) else core.agentSaved(device.id)
    }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onClose, sheetState = sheetState, containerColor = MaterialTheme.colorScheme.surfaceContainerHigh) {
        Column(
            modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 8.dp).padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Column(Modifier.padding(horizontal = 12.dp).padding(bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(stringResource(R.string.agents_share_label), style = MaterialTheme.typography.titleMedium)
                if (pc != null) {
                    Text(pc.name, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            when {
                pcs.isEmpty() -> Unit
                pc == null -> PcChoices(pcs) { chosen = it }
                else -> Destinations(
                    device = pc,
                    agents = all[pc.id] ?: AgentsState(),
                    placesOffered = remember(pc.id, pc.online) { core.agentPlacesOffered(pc.id) },
                    incoming = incoming,
                    pictures = pictures,
                    onPicked = { threadId -> onPicked(pc.id, threadId, pc.online, incoming.text) },
                )
            }
        }
    }
}

@Composable
private fun PcChoices(pcs: List<Device>, onPick: (String) -> Unit) {
    for (device in pcs) {
        G7Row(
            label = device.name,
            icon = AgentGlyphs.laptop,
            sublabel = if (device.online) null else stringResource(R.string.agents_offline_unsaved, device.name),
            onClick = { onPick(device.id) },
        )
    }
}

@Composable
private fun Destinations(
    device: Device,
    agents: AgentsState,
    placesOffered: Boolean,
    incoming: Incoming,
    pictures: SharePictures?,
    onPicked: (String?) -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val catalog = agents.catalog
    val online = device.online
    val reading = incoming.streams.isNotEmpty() && pictures == null
    val images = if (online) pictures?.added?.size ?: 0 else 0
    // Offline, only text can wait in a thread's box; pictures need the PC.
    val something = incoming.text != null || images > 0
    if (!online) Note(stringResource(R.string.agents_offline_unsaved, device.name), tints.amber)
    pictures?.refused?.forEach { (name, why) ->
        Note(
            when (why) {
                ShareRefusal.NOT_A_PICTURE -> stringResource(R.string.agents_share_refused, name)
                ShareRefusal.TOO_LARGE -> stringResource(R.string.agents_share_too_large, name)
                ShareRefusal.TOO_MANY -> stringResource(R.string.agents_share_too_many)
            },
            scheme.onSurfaceVariant,
        )
    }
    if (reading) {
        Box(Modifier.fillMaxWidth().padding(12.dp), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
        }
        return
    }
    // The folder is chosen in the new-thread flow (contract C §5.6); an older PC that can't be browsed still needs one.
    val canStart = online && catalog != null && agents.readyAgents.isNotEmpty() && (catalog.folders.isNotEmpty() || placesOffered)
    G7Row(
        label = stringResource(R.string.agents_new_thread),
        icon = G7Icons.plus,
        enabled = canStart && something,
        onClick = { onPicked(null) },
    )
    val threads = AgentShareRules.threads(agents)
    if (threads.isEmpty()) return
    G7SectionLabel(stringResource(R.string.agents_share_add_to))
    for (thread in threads) {
        Column(Modifier.fillMaxWidth()) {
            G7Row(
                label = thread.title,
                enabled = something,
                onClick = { onPicked(thread.id) },
            )
            StatusLine(
                status = AgentsModel.status(thread),
                trailing = listOf(agents.agentName(thread.agent), thread.folder).filter { it.isNotBlank() }.joinToString(" · "),
                modifier = Modifier.padding(start = 12.dp, end = 12.dp, bottom = 6.dp),
            )
        }
    }
}

@Composable
private fun Note(text: String, color: androidx.compose.ui.graphics.Color) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = color, modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp))
}
