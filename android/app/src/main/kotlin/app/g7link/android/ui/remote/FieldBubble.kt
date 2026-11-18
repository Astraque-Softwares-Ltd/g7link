// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.graphics.Bitmap
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.g7Appear
import app.g7link.android.ui.theme.g7Disappear
import app.g7link.core.TextFieldKind
import app.g7link.core.TextFieldState

/** The PC's field state, as the typing box works with it. */
internal fun TextFieldState.toPcField() = PcField(
    focus = focus.toInt(),
    field = field.toLong(),
    epoch = epoch.toInt(),
    applied = applied.toInt(),
    kind = when (kind) {
        TextFieldKind.MIRROR -> FieldKind.Mirror
        TextFieldKind.OPAQUE -> FieldKind.Opaque
        TextFieldKind.SECRET -> FieldKind.Secret
        TextFieldKind.NONE -> FieldKind.None
    },
    app = app,
    label = label,
    window = window,
    multiline = multiline,
    start = start.toInt(),
    before = before,
    after = after,
    anchor = anchor?.toInt(),
    icon = icon,
)

/**
 * An app's name as the PC gave it, readable: the last part of a reverse
 * domain name ("org.gnome.Ptyxis"), words capitalised ("claude-desktop").
 */
internal fun readableAppName(raw: String): String {
    val name = if (' ' !in raw && raw.count { it == '.' } >= 2) raw.substringAfterLast('.') else raw
    return name.split('-', '_', ' ')
        .filter { it.isNotEmpty() }
        .joinToString(" ") { word -> word.replaceFirstChar { it.titlecase() } }
        .ifEmpty { raw }
}

/** What the bubble names for a field that takes text. */
internal data class BubbleNames(val app: String?, val label: String?)

/** How window titles join the page or document to the app's name. */
private val TITLE_SEPARATORS = listOf(" - ", " – ", " — ", " | ")

/**
 * The names the bubble shows for a field, from the [app] and [label] the PC
 * sent: the label only when it adds something to the app's name (a window's
 * title loses the app's name at its end or start: "New Tab - Google Chrome"
 * is "New Tab"), and the app's name only when its icon isn't shown. Both
 * null: the icon alone, or nothing known.
 */
internal fun bubbleNames(app: String?, label: String?, window: Boolean, iconShown: Boolean): BubbleNames {
    val name = app?.takeIf { it.isNotBlank() }?.let(::readableAppName)
    var text = label?.trim().orEmpty()
    if (name != null && window) {
        for (separator in TITLE_SEPARATORS) {
            text = when {
                text.endsWith(separator + name, ignoreCase = true) ->
                    text.dropLast(separator.length + name.length)
                text.startsWith(name + separator, ignoreCase = true) ->
                    text.drop(name.length + separator.length)
                else -> continue
            }.trim()
            break
        }
    }
    val shownLabel = text.takeIf { it.isNotEmpty() && !it.equals(name, ignoreCase = true) }
    return BubbleNames(name.takeUnless { iconShown }, shownLabel)
}

/** What the bubble shows: the PC's field, and whether what's typed may differ there. */
private data class Bubble(val shown: PcField, val unverified: Boolean)

/**
 * Over the typing box: where on the PC what's typed goes (the app's icon and
 * "Search"), changing as the PC's focus moves from one field to another.
 * While what's typed may differ from the PC's field ([unverified]), it says
 * so, with a way to start afresh.
 */
@Composable
internal fun FieldBubble(
    field: PcField?,
    pcName: String,
    unverified: Boolean,
    onStartAfresh: () -> Unit,
    loadIcon: suspend (String) -> Bitmap?,
    modifier: Modifier = Modifier,
) {
    val enter = g7Appear()
    val exit = g7Disappear()
    // A new loader each time the box recomposes mustn't fetch the icon again.
    val load by rememberUpdatedState(loadIcon)
    AnimatedContent(
        targetState = field?.let { Bubble(it, unverified && it.kind == FieldKind.Opaque) },
        contentKey = { it?.shown?.field to it?.shown?.kind },
        transitionSpec = { enter togetherWith exit },
        label = "FieldBubble",
        modifier = modifier,
    ) { bubble ->
        if (bubble == null) return@AnimatedContent
        val shown = bubble.shown
        val scheme = MaterialTheme.colorScheme
        val takesText = shown.kind == FieldKind.Mirror || shown.kind == FieldKind.Opaque
        // The app's icon once it's here; until then, the field's glyph.
        val iconKey = shown.icon?.takeIf { takesText }
        val loaded by produceState<Pair<String, ImageBitmap>?>(null, iconKey) {
            val key = iconKey ?: return@produceState
            load(key)?.let { value = key to it.asImageBitmap() }
        }
        val appIcon = loaded?.takeIf { it.first == iconKey }?.second
        val (app, label) = bubbleNames(shown.app, shown.label, shown.window, iconShown = appIcon != null)
        val text = when (shown.kind) {
            FieldKind.Secret -> stringResource(R.string.remote_field_secret)
            FieldKind.None -> stringResource(R.string.remote_field_none, pcName)
            FieldKind.Mirror, FieldKind.Opaque -> when {
                app != null && label != null -> stringResource(R.string.remote_field_label, app, label)
                appIcon != null -> label
                else -> app ?: label ?: pcName
            }
        }
        val glyph = when (shown.kind) {
            FieldKind.Mirror -> G7Icons.text
            FieldKind.Opaque -> if (shown.window) G7Icons.window else G7Icons.keyboard
            FieldKind.Secret -> G7Icons.lock
            FieldKind.None -> null
        }
        Row(
            // Room above, so the bubble never crowds the keys over it.
            modifier = Modifier.padding(start = 16.dp, top = 6.dp, end = 16.dp, bottom = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (appIcon != null) {
                Image(
                    appIcon,
                    contentDescription = shown.app?.takeIf { it.isNotBlank() }?.let(::readableAppName),
                    modifier = Modifier.size(16.dp).clip(RoundedCornerShape(4.dp)),
                )
            } else if (glyph != null) {
                Icon(glyph, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(14.dp))
            }
            if (text != null) {
                Text(
                    text,
                    style = MaterialTheme.typography.labelMedium,
                    color = if (shown.kind == FieldKind.None) scheme.onSurfaceVariant else scheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.MiddleEllipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
            }
            if (bubble.unverified) {
                Text(
                    stringResource(R.string.remote_field_may_differ, pcName),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                Text(
                    stringResource(R.string.remote_field_start_afresh),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.primary,
                    maxLines = 1,
                    modifier = Modifier.clickable(role = Role.Button, onClick = onStartAfresh),
                )
            }
        }
    }
}
