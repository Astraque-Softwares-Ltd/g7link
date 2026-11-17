// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
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

/**
 * Over the typing box: where on the PC what's typed goes ("Firefox · Search"),
 * changing as the PC's focus moves from one field to another.
 */
@Composable
internal fun FieldBubble(field: PcField?, pcName: String, modifier: Modifier = Modifier) {
    val enter = g7Appear()
    val exit = g7Disappear()
    AnimatedContent(
        targetState = field,
        contentKey = { it?.field to it?.kind },
        transitionSpec = { enter togetherWith exit },
        label = "FieldBubble",
        modifier = modifier,
    ) { shown ->
        if (shown == null) return@AnimatedContent
        val scheme = MaterialTheme.colorScheme
        val app = shown.app?.let(::readableAppName)
        val label = shown.label?.takeIf { it.isNotBlank() }
        val text = when (shown.kind) {
            FieldKind.Secret -> stringResource(R.string.remote_field_secret)
            FieldKind.None -> stringResource(R.string.remote_field_none, pcName)
            FieldKind.Mirror, FieldKind.Opaque -> when {
                app != null && label != null -> stringResource(R.string.remote_field_label, app, label)
                app != null -> app
                label != null -> label
                else -> pcName
            }
        }
        val icon = when (shown.kind) {
            FieldKind.Mirror -> G7Icons.text
            FieldKind.Opaque -> if (shown.window) G7Icons.window else G7Icons.keyboard
            FieldKind.Secret -> G7Icons.lock
            FieldKind.None -> null
        }
        Row(
            modifier = Modifier.padding(start = 16.dp, end = 16.dp, bottom = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (icon != null) {
                Icon(icon, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(14.dp))
            }
            Text(
                text,
                style = MaterialTheme.typography.labelMedium,
                color = if (shown.kind == FieldKind.None) scheme.onSurfaceVariant else scheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
            )
        }
    }
}
