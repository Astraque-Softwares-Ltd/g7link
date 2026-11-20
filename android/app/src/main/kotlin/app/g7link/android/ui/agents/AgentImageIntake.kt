// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import java.io.File
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** A picture read for sending: its name and bytes, or why it can't go. */
sealed interface ImageRead {
    data class Bytes(val name: String, val bytes: ByteArray) : ImageRead {
        override fun equals(other: Any?): Boolean = other is Bytes && other.name == name && other.bytes.contentEquals(bytes)

        override fun hashCode(): Int = 31 * name.hashCode() + bytes.contentHashCode()
    }

    data class Problem(val problem: ImageProblem) : ImageRead
}

/**
 * Pictures for a message (contract §Q6.3): from the system photo picker or a
 * photo taken into the app's cache, read whole but never past the wire's 10
 * MiB, and checked to be one of the four types the PC takes. The PC decodes
 * and re-encodes them; nothing here decodes what the user picked.
 */
object AgentImageIntake {
    private const val TAG = "AgentImages"

    /** Photos taken for a message wait here until they're sent, then go. */
    private const val CAMERA_DIR = "agent-camera"
    private const val FALLBACK_NAME = "image"
    private const val MAX_NAME_CHARS = 128

    /** Reads [uri] for sending; on the IO dispatcher. */
    suspend fun read(context: Context, uri: Uri): ImageRead = withContext(Dispatchers.IO) {
        runCatching {
            val resolver = context.contentResolver
            val name = displayName(context, uri)
            val bytes = resolver.openInputStream(uri)?.use { input ->
                val out = java.io.ByteArrayOutputStream()
                val buffer = ByteArray(64 * 1024)
                var total = 0
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    total += read
                    // One byte past the limit is enough to know it's too large.
                    if (total > AgentImages.MAX_BYTES) return@runCatching ImageRead.Problem(ImageProblem.TOO_LARGE)
                    out.write(buffer, 0, read)
                }
                out.toByteArray()
            } ?: return@runCatching ImageRead.Problem(ImageProblem.FAILED)
            AgentImages.problem(bytes)?.let { ImageRead.Problem(it) } ?: ImageRead.Bytes(name, bytes)
        }.getOrElse {
            Log.w(TAG, "can't read the picture", it)
            ImageRead.Problem(ImageProblem.FAILED)
        }
    }

    /** The picture's own file name when the provider says, as a plain name. */
    private fun displayName(context: Context, uri: Uri): String {
        val name = runCatching {
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0) else null
            }
        }.getOrNull()
        val plain = name.orEmpty().substringAfterLast('/').filter { !it.isISOControl() }.take(MAX_NAME_CHARS).trim()
        return plain.ifEmpty { FALLBACK_NAME }
    }

    /** A new file in the cache for the camera to write into, and its URI for the camera app. */
    fun cameraFile(context: Context): Pair<File, Uri>? = runCatching {
        val dir = File(context.cacheDir, CAMERA_DIR)
        check(dir.isDirectory || dir.mkdirs())
        val file = File(dir, "photo-${UUID.randomUUID()}.jpg")
        file to FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    }.getOrElse {
        Log.w(TAG, "can't make a file for the camera", it)
        null
    }

    /** A photo taken for a message, once it's sent or dropped. */
    fun deleteCameraFile(path: String?) {
        if (path.isNullOrEmpty()) return
        runCatching { File(path).delete() }
    }

    /** Photos left from a run that ended before they were sent. */
    fun sweepCamera(context: Context) {
        runCatching { File(context.cacheDir, CAMERA_DIR).listFiles()?.forEach { it.delete() } }
    }
}

/**
 * Thumbnails of images the PC holds, by id, read with `agent.image.thumb`
 * (a JPEG of at most 150 KiB) and kept while the thread is open.
 */
@Stable
class ImageThumbs(private val fetch: suspend (String) -> ByteArray?) {
    private val lock = Mutex()
    private val kept = LinkedHashMap<String, ImageBitmap>()
    private val failed = HashSet<String>()

    suspend fun load(id: String): ImageBitmap? {
        lock.withLock {
            kept[id]?.let { return it }
            if (id in failed) return null
        }
        val bytes = fetch(id)?.takeIf { AgentImages.isThumb(it) }
        val bitmap = bytes?.let { decode(it) }
        lock.withLock {
            if (bitmap == null) {
                failed += id
            } else {
                if (kept.size >= MAX_KEPT) kept.keys.firstOrNull()?.let(kept::remove)
                kept[id] = bitmap
            }
        }
        return bitmap
    }

    /** Decodes the PC's JPEG, at most [MAX_SIDE] a side. */
    private suspend fun decode(bytes: ByteArray): ImageBitmap? = withContext(Dispatchers.Default) {
        runCatching {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return@runCatching null
            var sample = 1
            while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_SIDE) sample *= 2
            val options = BitmapFactory.Options().apply { inSampleSize = sample }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.asImageBitmap()
        }.getOrNull()
    }

    private companion object {
        const val MAX_KEPT = 32
        const val MAX_SIDE = 1024
    }
}

/** What opens the photo picker and the camera for the reply box. */
@Stable
class ImagePickers(
    /** Opens the system photo picker for up to [room] pictures. */
    val gallery: (room: Int) -> Unit,
    /** Takes a photo, asking for the camera first when it must. */
    val camera: () -> Unit,
)

/**
 * The photo picker and the camera. [onPicked] gets the pictures chosen, and
 * for a photo taken, the cache file to delete once it's sent or dropped.
 */
@Composable
fun rememberImagePickers(onPicked: (List<Uri>, String?) -> Unit, onCameraFailed: () -> Unit): ImagePickers {
    val context = LocalContext.current
    val picked by rememberUpdatedState(onPicked)
    val cameraFailed by rememberUpdatedState(onCameraFailed)
    var room by remember { mutableStateOf(AgentImages.MAX_PER_MESSAGE) }
    // The file the camera writes into survives the app being recreated while it's open.
    var cameraPath by rememberSaveable { mutableStateOf<String?>(null) }
    var cameraUri by rememberSaveable { mutableStateOf<String?>(null) }
    val one = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) picked(listOf(uri), null)
    }
    val many = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(AgentImages.MAX_PER_MESSAGE)) { uris ->
        if (uris.isNotEmpty()) picked(uris.take(room), null)
    }
    val take = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { saved ->
        val path = cameraPath
        val uri = cameraUri?.let(Uri::parse)
        cameraPath = null
        cameraUri = null
        if (saved && path != null && uri != null) {
            picked(listOf(uri), path)
        } else {
            AgentImageIntake.deleteCameraFile(path)
        }
    }
    val launchCamera: () -> Unit = {
        val made = AgentImageIntake.cameraFile(context)
        if (made == null) {
            cameraFailed()
        } else {
            cameraPath = made.first.absolutePath
            cameraUri = made.second.toString()
            val launched = runCatching { take.launch(made.second) }.isSuccess
            if (!launched) {
                AgentImageIntake.deleteCameraFile(cameraPath)
                cameraPath = null
                cameraUri = null
                cameraFailed()
            }
        }
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) launchCamera()
    }
    return remember(one, many, take, permission) {
        ImagePickers(
            gallery = { free ->
                room = free.coerceIn(0, AgentImages.MAX_PER_MESSAGE)
                val request = PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)
                when {
                    free <= 0 -> Unit
                    free == 1 -> runCatching { one.launch(request) }
                    else -> runCatching { many.launch(request) }
                }
            },
            camera = {
                val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
                if (granted) launchCamera() else runCatching { permission.launch(Manifest.permission.CAMERA) }
            },
        )
    }
}
