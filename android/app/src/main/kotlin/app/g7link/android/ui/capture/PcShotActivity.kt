// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.capture

import android.content.ClipData
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.BitmapRegionDecoder
import android.graphics.Rect
import android.media.MediaScannerConnection
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.G7LinkApplication
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.PcCapture
import app.g7link.android.core.PcShot
import app.g7link.android.ui.Preferences
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7LinkTheme
import app.g7link.android.ui.theme.G7RaisedButton
import java.io.ByteArrayOutputStream
import java.io.File
import kotlin.math.roundToInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * A screenshot of a PC, just taken from this phone: crop it (the whole screen to
 * start with), then save it to Pictures/G7 Link, share it, or copy it to the PC.
 */
class PcShotActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val file = intent.getStringExtra(EXTRA_FILE)?.let(::File)?.takeIf(::isShot)
        val pcId = intent.getStringExtra(EXTRA_PC)
        if (file == null || pcId == null) {
            finish()
            return
        }
        val pcName = intent.getStringExtra(EXTRA_PC_NAME).orEmpty()
        val core = (application as G7LinkApplication).core
        val preferences = Preferences(this)
        setContent {
            val appearance by preferences.appearance.collectAsStateWithLifecycle()
            G7LinkTheme(appearance) {
                PcShotScreen(core = core, pcId = pcId, pcName = pcName, file = file, onClose = ::finish)
            }
        }
    }

    /** Only a screenshot this app keeps, never a path from elsewhere. */
    private fun isShot(file: File): Boolean = runCatching {
        file.isFile && file.canonicalFile.parentFile == PcCapture.shotsDir(this).canonicalFile
    }.getOrDefault(false)

    companion object {
        private const val EXTRA_FILE = "pc_shot_file"
        private const val EXTRA_PC = "pc_shot_pc"
        private const val EXTRA_PC_NAME = "pc_shot_pc_name"

        fun intent(context: Context, shot: PcShot): Intent =
            Intent(context, PcShotActivity::class.java)
                .putExtra(EXTRA_FILE, shot.file.path)
                .putExtra(EXTRA_PC, shot.pcId)
                .putExtra(EXTRA_PC_NAME, shot.pcName)
    }
}

/** A screenshot shown to crop: a picture small enough to draw, and the full size. */
private class Loaded(val image: ImageBitmap, val width: Int, val height: Int)

@Composable
private fun PcShotScreen(core: Core, pcId: String, pcName: String, file: File, onClose: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var loaded by remember { mutableStateOf<Loaded?>(null) }
    var unreadable by remember { mutableStateOf(false) }
    var crop by remember { mutableStateOf(CropRect.FULL) }
    var working by remember { mutableStateOf(false) }
    val name = remember(file) { file.name.substringAfter('-') }
    LaunchedEffect(file) {
        val picture = withContext(Dispatchers.IO) { ShotFiles.load(file) }
        if (picture == null) unreadable = true else loaded = picture
    }

    /** Crops on a background thread, then does [use] with the PNG; says so when it can't. */
    fun withPng(use: suspend (ByteArray) -> String?) {
        val picture = loaded ?: return
        if (working) return
        working = true
        val selection = crop
        scope.launch {
            val png = withContext(Dispatchers.IO) { ShotFiles.cropPng(file, selection, picture.width, picture.height) }
            val message = if (png == null) context.getString(R.string.pc_shot_unreadable) else use(png)
            working = false
            if (message != null) snackbar.showSnackbar(message)
        }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            G7Header(title = stringResource(R.string.pc_shot_title), onBack = onClose, note = pcName)
            Box(
                Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                contentAlignment = Alignment.Center,
            ) {
                val picture = loaded
                when {
                    picture != null -> CropView(
                        image = picture.image,
                        crop = crop,
                        onCrop = { crop = it },
                        description = stringResource(R.string.pc_shot_crop_desc, pcName),
                        modifier = Modifier.fillMaxSize(),
                    )
                    unreadable -> Text(
                        stringResource(R.string.pc_shot_unreadable),
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    else -> CircularProgressIndicator()
                }
            }
            val ready = loaded != null && !working
            Column(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    G7RaisedButton(
                        label = stringResource(R.string.pc_shot_save),
                        enabled = ready,
                        modifier = Modifier.weight(1f),
                        onClick = {
                            withPng { png ->
                                val saved = withContext(Dispatchers.IO) { ShotFiles.saveToPictures(context, png, name) }
                                context.getString(if (saved) R.string.pc_shot_saved else R.string.pc_shot_not_saved)
                            }
                        },
                    )
                    G7RaisedButton(
                        label = stringResource(R.string.pc_shot_share),
                        enabled = ready,
                        modifier = Modifier.weight(1f),
                        onClick = {
                            withPng { png ->
                                val shared = withContext(Dispatchers.IO) { ShotFiles.forSharing(context, png, name) }
                                if (shared == null) {
                                    context.getString(R.string.pc_shot_not_saved)
                                } else {
                                    ShotFiles.share(context, shared, context.getString(R.string.pc_shot_share_title))
                                    null
                                }
                            }
                        },
                    )
                }
                G7RaisedButton(
                    label = stringResource(R.string.pc_shot_copy, pcName),
                    enabled = ready,
                    modifier = Modifier.fillMaxWidth(),
                    onClick = { withPng { png -> core.sendClipboardPng(pcId, png) } },
                )
            }
        }
    }
}

/** The screenshot, fitted in its box, with the crop drawn over it: drag a corner, an edge, or the inside. */
@Composable
private fun CropView(
    image: ImageBitmap,
    crop: CropRect,
    onCrop: (CropRect) -> Unit,
    description: String,
    modifier: Modifier = Modifier,
) {
    val accent = MaterialTheme.colorScheme.primary
    val currentCrop by rememberUpdatedState(crop)
    val currentOnCrop by rememberUpdatedState(onCrop)
    val density = LocalDensity.current
    val slop = with(density) { 24.dp.toPx() }
    val minSide = with(density) { 48.dp.toPx() }
    val line = with(density) { 2.dp.toPx() }
    val handle = with(density) { 4.dp.toPx() }
    val arm = with(density) { 18.dp.toPx() }
    BoxWithConstraints(modifier) {
        val fit = CropMath.fit(
            constraints.maxWidth.toFloat(),
            constraints.maxHeight.toFloat(),
            image.width.toFloat(),
            image.height.toFloat(),
        )
        Canvas(
            Modifier
                .fillMaxSize()
                .semantics { contentDescription = description }
                .pointerInput(fit) {
                    if (fit.width <= 0f || fit.height <= 0f) return@pointerInput
                    var grabbed: CropHandle? = null
                    detectDragGestures(
                        onDragStart = { at ->
                            grabbed = CropMath.handleAt(
                                currentCrop,
                                (at.x - fit.left) / fit.width,
                                (at.y - fit.top) / fit.height,
                                slop / fit.width,
                                slop / fit.height,
                            )
                        },
                        onDragEnd = { grabbed = null },
                        onDragCancel = { grabbed = null },
                    ) { change, drag ->
                        val held = grabbed ?: return@detectDragGestures
                        change.consume()
                        currentOnCrop(
                            CropMath.drag(
                                currentCrop,
                                held,
                                drag.x / fit.width,
                                drag.y / fit.height,
                                minSide / fit.width,
                                minSide / fit.height,
                            ),
                        )
                    }
                },
        ) {
            drawImage(
                image,
                dstOffset = IntOffset(fit.left.roundToInt(), fit.top.roundToInt()),
                dstSize = IntSize(fit.width.roundToInt(), fit.height.roundToInt()),
            )
            val left = fit.left + crop.left * fit.width
            val top = fit.top + crop.top * fit.height
            val right = fit.left + crop.right * fit.width
            val bottom = fit.top + crop.bottom * fit.height
            // What's left out, dimmed.
            val shade = Color.Black.copy(alpha = 0.55f)
            drawRect(shade, Offset(fit.left, fit.top), Size(fit.width, top - fit.top))
            drawRect(shade, Offset(fit.left, bottom), Size(fit.width, fit.top + fit.height - bottom))
            drawRect(shade, Offset(fit.left, top), Size(left - fit.left, bottom - top))
            drawRect(shade, Offset(right, top), Size(fit.left + fit.width - right, bottom - top))
            drawRect(accent, Offset(left, top), Size(right - left, bottom - top), style = Stroke(width = line))
            // The corners, to grab.
            for ((x, y, toward) in listOf(
                Triple(left, top, Offset(1f, 1f)),
                Triple(right, top, Offset(-1f, 1f)),
                Triple(left, bottom, Offset(1f, -1f)),
                Triple(right, bottom, Offset(-1f, -1f)),
            )) {
                drawLine(accent, Offset(x, y), Offset(x + toward.x * arm, y), strokeWidth = handle)
                drawLine(accent, Offset(x, y), Offset(x, y + toward.y * arm), strokeWidth = handle)
            }
        }
    }
}

/** Reading, cropping and keeping screenshots of a PC. */
private object ShotFiles {
    private const val TAG = "PcShot"
    /** The longest side the picture is drawn at; the crop itself comes from the full one. */
    private const val MAX_PREVIEW = 2560
    private const val ALBUM = "G7 Link"

    fun load(file: File): Loaded? = runCatching {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(file.path, bounds)
        val width = bounds.outWidth
        val height = bounds.outHeight
        if (width <= 0 || height <= 0) return null
        var sample = 1
        while (maxOf(width, height) / sample > MAX_PREVIEW) sample *= 2
        val bitmap = BitmapFactory.decodeFile(file.path, BitmapFactory.Options().apply { inSampleSize = sample })
            ?: return null
        Loaded(bitmap.asImageBitmap(), width, height)
    }.getOrElse {
        Log.w(TAG, "can't read the screenshot", it)
        null
    }

    /** The crop of [file] as a PNG: the file itself when it's the whole picture. */
    fun cropPng(file: File, crop: CropRect, width: Int, height: Int): ByteArray? = runCatching {
        val px = CropMath.pixels(crop, width, height)
        if (px.left == 0 && px.top == 0 && px.right == width && px.bottom == height) return file.readBytes()
        val region = decodeRegion(file, Rect(px.left, px.top, px.right, px.bottom)) ?: return null
        try {
            ByteArrayOutputStream().also { region.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
        } finally {
            region.recycle()
        }
    }.getOrElse {
        Log.w(TAG, "can't crop the screenshot", it)
        null
    }

    private fun openDecoder(file: File): BitmapRegionDecoder? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            BitmapRegionDecoder.newInstance(file.path)
        } else {
            @Suppress("DEPRECATION")
            BitmapRegionDecoder.newInstance(file.path, false)
        }

    private fun decodeRegion(file: File, rect: Rect): Bitmap? {
        val decoder = openDecoder(file) ?: return null
        return try {
            decoder.decodeRegion(rect, null)
        } finally {
            decoder.recycle()
        }
    }

    /** Saves to Pictures/G7 Link, where the gallery finds it. */
    fun saveToPictures(context: Context, png: ByteArray, name: String): Boolean = runCatching {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = context.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, name)
                put(MediaStore.MediaColumns.MIME_TYPE, "image/png")
                put(MediaStore.MediaColumns.RELATIVE_PATH, "${Environment.DIRECTORY_PICTURES}/$ALBUM")
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
            val uri = checkNotNull(resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values))
            try {
                checkNotNull(resolver.openOutputStream(uri)).use { it.write(png) }
                resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            } catch (e: Exception) {
                resolver.delete(uri, null, null)
                throw e
            }
        } else {
            // Android 8–9: needs the storage permission, which Storage in Settings asks for.
            @Suppress("DEPRECATION")
            val pictures = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_PICTURES)
            val dir = File(pictures, ALBUM)
            check(dir.isDirectory || dir.mkdirs())
            var target = File(dir, name)
            var n = 2
            while (target.exists()) {
                target = File(dir, "${name.substringBeforeLast('.')} ($n).png")
                n++
            }
            target.writeBytes(png)
            MediaScannerConnection.scanFile(context, arrayOf(target.path), arrayOf("image/png"), null)
        }
        true
    }.getOrElse {
        Log.w(TAG, "can't save the screenshot", it)
        false
    }

    /** A copy other apps may read, for the share sheet; earlier ones go. */
    fun forSharing(context: Context, png: ByteArray, name: String): File? = runCatching {
        val dir = File(PcCapture.shotsDir(context), "shared")
        dir.listFiles()?.forEach { it.delete() }
        check(dir.isDirectory || dir.mkdirs())
        File(dir, name).apply { writeBytes(png) }
    }.getOrNull()

    fun share(context: Context, file: File, title: String) {
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
        val send = Intent(Intent.ACTION_SEND)
            .setType("image/png")
            .putExtra(Intent.EXTRA_STREAM, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        send.clipData = ClipData.newRawUri(file.name, uri)
        runCatching { context.startActivity(Intent.createChooser(send, title)) }
    }
}
