// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.text.format.Formatter
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.Saver
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.clipboard.PhoneClipboard
import app.g7link.android.ui.theme.G7Icons
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Something picked to send with what's typed: a picture or a file. */
internal data class Attachment(val uri: String, val name: String, val size: Long, val mime: String) {
    val isImage: Boolean get() = mime.startsWith("image/")

    companion object {
        /** Most attachments waiting at once. */
        const val MAX = 10

        /** What a content [uri] is called, how big and what it is (blocking; off the main thread). */
        fun describe(context: Context, uri: Uri): Attachment? = runCatching {
            var name = uri.lastPathSegment ?: "file"
            var size = -1L
            context.contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
                null,
                null,
                null,
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    cursor.getString(0)?.let { name = it }
                    if (!cursor.isNull(1)) size = cursor.getLong(1)
                }
            }
            val mime = context.contentResolver.getType(uri) ?: "application/octet-stream"
            Attachment(uri.toString(), name, size, mime)
        }.getOrNull()

        /** Kept across rotation as plain strings. */
        val listSaver: Saver<List<Attachment>, Any> = Saver(
            save = { list -> ArrayList(list.flatMap { listOf(it.uri, it.name, it.size.toString(), it.mime) }) },
            restore = { saved ->
                @Suppress("UNCHECKED_CAST")
                (saved as List<String>).chunked(4).mapNotNull { (uri, name, size, mime) ->
                    size.toLongOrNull()?.let { Attachment(uri, name, it, mime) }
                }
            },
        )
    }
}

/**
 * Over the typing box: what's waiting to go with the next Send, a small
 * picture for each image and the name and size of each file, each with a
 * cross to take it out.
 */
@Composable
internal fun AttachmentTray(items: List<Attachment>, onRemove: (Attachment) -> Unit, modifier: Modifier = Modifier) {
    LazyRow(
        modifier = modifier,
        contentPadding = PaddingValues(horizontal = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(items, key = { it.uri }) { item ->
            Box {
                if (item.isImage) ImageTile(item) else FileTile(item)
                val remove = stringResource(R.string.remote_attachment_remove, item.name)
                Box(
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .size(32.dp)
                        .clickable(onClick = { onRemove(item) })
                        .semantics { contentDescription = remove },
                    contentAlignment = Alignment.TopEnd,
                ) {
                    Box(
                        Modifier
                            .padding(3.dp)
                            .size(20.dp)
                            .clip(CircleShape)
                            .background(MaterialTheme.colorScheme.inverseSurface),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            G7Icons.close,
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.inverseOnSurface,
                            modifier = Modifier.size(12.dp),
                        )
                    }
                }
            }
        }
    }
}

private val TILE = 56.dp

@Composable
private fun ImageTile(item: Attachment) {
    val context = LocalContext.current
    val px = with(LocalDensity.current) { TILE.roundToPx() }
    val picture by produceState<android.graphics.Bitmap?>(null, item.uri) {
        value = withContext(Dispatchers.IO) {
            PhoneClipboard.previewImage(context, Uri.parse(item.uri), px, px)?.picture
        }
    }
    Box(
        Modifier
            .size(TILE)
            .clip(RoundedCornerShape(12.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant),
    ) {
        picture?.let {
            Image(
                it.asImageBitmap(),
                contentDescription = item.name,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}

@Composable
private fun FileTile(item: Attachment) {
    val context = LocalContext.current
    Row(
        modifier = Modifier
            .height(TILE)
            .widthIn(max = 200.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(start = 10.dp, end = 26.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(G7Icons.folder, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(20.dp))
        Column {
            Text(
                item.name,
                style = MaterialTheme.typography.labelMedium,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
            )
            if (item.size >= 0) {
                Text(
                    Formatter.formatShortFileSize(context, item.size),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
