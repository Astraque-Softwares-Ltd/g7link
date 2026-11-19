// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.FileProvider
import app.g7link.core.AgentExport
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Share for a thread or a followed session: its Markdown in the Android share
 * sheet, as text, or past [MAX_TEXT_BYTES] (more than many apps take as
 * text) as a `.md` file in the cache that only the chosen app may read. Only
 * the last shared file is kept.
 */
object AgentShare {
    /** Longer Markdown goes as a file. */
    const val MAX_TEXT_BYTES = 100_000

    private const val TAG = "AgentShare"
    private const val DIR = "agent-exports"
    private const val FALLBACK_NAME = "transcript.md"
    private const val MAX_NAME_CHARS = 128

    /** Whether [text] goes as a file rather than as text. */
    fun asFile(text: String): Boolean =
        text.length > MAX_TEXT_BYTES || text.encodeToByteArray().size > MAX_TEXT_BYTES

    /** The PC's suggested name as a plain `.md` file name in the cache. */
    fun fileName(suggested: String): String {
        val name = suggested
            .filter { (it in 'a'..'z') || (it in 'A'..'Z') || (it in '0'..'9') || it == '.' || it == '_' || it == '-' }
            .trimStart('.')
            .take(MAX_NAME_CHARS)
        return when {
            name.isEmpty() -> FALLBACK_NAME
            name.endsWith(".md") -> name
            else -> "$name.md"
        }
    }

    /** Opens the share sheet for [export], titled [title]; false when it couldn't. */
    suspend fun share(context: Context, export: AgentExport, title: String): Boolean {
        val send = Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_SUBJECT, title)
        if (!asFile(export.text)) {
            send.setType("text/plain").putExtra(Intent.EXTRA_TEXT, export.text)
        } else {
            val name = fileName(export.fileName)
            val file = withContext(Dispatchers.IO) { writeFile(context, name, export.text) } ?: return false
            val uri = runCatching { FileProvider.getUriForFile(context, "${context.packageName}.files", file) }
                .getOrNull() ?: return false
            send.setType("text/markdown")
                .putExtra(Intent.EXTRA_STREAM, uri)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            send.clipData = ClipData.newRawUri(file.name, uri)
        }
        return runCatching { context.startActivity(Intent.createChooser(send, null)) }
            .onFailure { Log.w(TAG, "no share sheet", it) }
            .isSuccess
    }

    private fun writeFile(context: Context, name: String, text: String): File? = runCatching {
        val dir = File(context.cacheDir, DIR)
        dir.listFiles()?.forEach { it.delete() }
        check(dir.isDirectory || dir.mkdirs())
        File(dir, name).apply { writeText(text) }
    }.getOrElse {
        Log.w(TAG, "can't write the transcript to share", it)
        null
    }
}
