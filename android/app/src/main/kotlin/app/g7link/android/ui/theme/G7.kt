// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R

/**
 * G7's "Quiet Tool" colors (docs/design-language.md in G7 Linux): charcoal grounds,
 * one lavender accent, and icons drawn in their own tints.
 */
object G7Colors {
    val darkPalette = Palette(
        primary = Color(0xFFB292D0),
        onPrimary = Color(0xFF18181B),
        primaryContainer = Color(0xFF2B2B30),
        onPrimaryContainer = Color(0xFFF2F2F3),
        secondaryContainer = Color(0xFF2B2B30),
        onSecondaryContainer = Color(0xFFF2F2F3),
        surface = Color(0xFF18181B),
        surfaceContainerLow = Color(0xFF18181B),
        surfaceContainer = Color(0xFF222226),
        surfaceContainerHigh = Color(0xFF2B2B30),
        surfaceContainerHighest = Color(0xFF303035),
        onSurface = Color(0xFFF2F2F3),
        onSurfaceVariant = Color(0xFFA1A1A8),
        outline = Color(0xFF6F6F78),
        outlineVariant = Color(0xFF303035),
    )

    val lightPalette = Palette(
        primary = Color(0xFF7D5CA9),
        onPrimary = Color(0xFFFAFAFB),
        primaryContainer = Color(0xFFE9E9ED),
        onPrimaryContainer = Color(0xFF1C1C1F),
        secondaryContainer = Color(0xFFE9E9ED),
        onSecondaryContainer = Color(0xFF1C1C1F),
        surface = Color(0xFFFAFAFB),
        surfaceContainerLow = Color(0xFFFAFAFB),
        surfaceContainer = Color(0xFFF1F1F3),
        surfaceContainerHigh = Color(0xFFE9E9ED),
        surfaceContainerHighest = Color(0xFFE4E4E9),
        onSurface = Color(0xFF1C1C1F),
        onSurfaceVariant = Color(0xFF5F5F67),
        outline = Color(0xFF7A7A83),
        outlineVariant = Color(0xFFDCDCE1),
    )

    val darkError = Color(0xFFE5736F)
    val lightError = Color(0xFFA4230F)

    /** The camera and desk screens sit on a deeper ground. */
    val deepGround = Color(0xFF0E0E10)
}

/** The icon tints. Each feature keeps its color wherever it appears. */
@Immutable
data class G7Tints(
    val blue: Color,
    val teal: Color,
    val coral: Color,
    val pink: Color,
    val amber: Color,
    val green: Color,
    val red: Color,
    val lavender: Color,
    val muted: Color,
) {
    companion object {
        val dark = G7Tints(
            blue = Color(0xFF7CB7EE),
            teal = Color(0xFF5DCAA5),
            coral = Color(0xFFF0997B),
            pink = Color(0xFFED93B1),
            amber = Color(0xFFEF9F27),
            green = Color(0xFF97C459),
            red = Color(0xFFE5736F),
            lavender = Color(0xFFB292D0),
            muted = Color(0xFFA1A1A8),
        )
        val light = G7Tints(
            blue = Color(0xFF2F74B5),
            teal = Color(0xFF1E8A68),
            coral = Color(0xFFC5532F),
            pink = Color(0xFFB8436E),
            amber = Color(0xFFA86400),
            green = Color(0xFF4F7F1A),
            red = Color(0xFFB8322E),
            lavender = Color(0xFF7D5CA9),
            muted = Color(0xFF5F5F67),
        )
    }
}

/** The tints for the current theme's lightness. */
@Composable
fun g7Tints(): G7Tints =
    if (MaterialTheme.colorScheme.surface.luminance() < 0.5f) G7Tints.dark else G7Tints.light

private fun strokeIcon(name: String, path: String): ImageVector =
    ImageVector.Builder(
        name = name,
        defaultWidth = 24.dp,
        defaultHeight = 24.dp,
        viewportWidth = 24f,
        viewportHeight = 24f,
    ).addPath(
        pathData = PathParser().parsePathString(path).toNodes(),
        fill = null,
        stroke = SolidColor(Color.White),
        strokeLineWidth = 1.7f,
        strokeLineCap = StrokeCap.Round,
        strokeLineJoin = StrokeJoin.Round,
    ).build()

/** Line icons on a 24 grid, drawn in a tint with nothing behind them. */
object G7Icons {
    val home by lazy { strokeIcon("home", "M3,11L12,4L21,11 M5,10V20H19V10") }
    val guide by lazy { strokeIcon("guide", "M4,5A2,2 0 0,1 6,3H19V19H6A2,2 0 0,0 4,21Z M4,5V21") }
    val settings by lazy {
        strokeIcon(
            "settings",
            "M12,9A3,3 0 1,0 12,15A3,3 0 1,0 12,9 M19,12H21 M3,12H5 M12,3V5 M12,19V21 M5.6,5.6L7,7 M17,17L18.4,18.4 M5.6,18.4L7,17 M17,7L18.4,5.6",
        )
    }
    val upload by lazy { strokeIcon("upload", "M12,16V4 M7,9L12,4L17,9 M4,20H20") }
    val download by lazy { strokeIcon("download", "M12,4V16 M7,11L12,16L17,11 M4,20H20") }
    val clipboard by lazy { strokeIcon("clipboard", "M9,4H15V7H9Z M7,5H5V21H19V5H17") }
    val link by lazy {
        strokeIcon(
            "link",
            "M10,14A4,4 0 0,0 16,14L19,11A4,4 0 0,0 13,5L12,6 M14,10A4,4 0 0,0 8,10L5,13A4,4 0 0,0 11,19L12,18",
        )
    }
    val pointer by lazy { strokeIcon("pointer", "M5,3L19,10L13,12L11,18Z") }
    val deck by lazy { strokeIcon("deck", "M4,4H20V20H4Z M4,12H20 M12,4V20") }
    val lock by lazy { strokeIcon("lock", "M6,11H18V20H6Z M8,11V7A4,4 0 0,1 16,7V11") }
    val power by lazy { strokeIcon("power", "M12,3V11 M6.3,6.3A8,8 0 1,0 17.7,6.3") }
    val camera by lazy { strokeIcon("camera", "M4,8H7L9,5H15L17,8H20V19H4Z M12,10A3,3 0 1,0 12,16A3,3 0 1,0 12,10") }
    val mic by lazy { strokeIcon("mic", "M9,4A3,3 0 0,1 15,4V11A3,3 0 0,1 9,11Z M5,11A7,7 0 0,0 19,11 M12,18V21") }
    val micOff by lazy { strokeIcon("micOff", "M9,4A3,3 0 0,1 15,4V11A3,3 0 0,1 9,11Z M5,11A7,7 0 0,0 19,11 M12,18V21 M4,4L20,20") }
    val clock by lazy { strokeIcon("clock", "M12,7V12L15,14 M12,3A9,9 0 1,0 12,21A9,9 0 1,0 12,3") }
    val bell by lazy { strokeIcon("bell", "M6,16V11A6,6 0 0,1 18,11V16L20,18H4Z M10,20A2,2 0 0,0 14,20") }
    val message by lazy { strokeIcon("message", "M4,5H20V16H8L4,20Z") }
    val phone by lazy {
        strokeIcon(
            "phone",
            "M5,4H9L11,9L8.5,10.5A11,11 0 0,0 13.5,15.5L15,13L20,15V19A1,1 0 0,1 19,20A16,16 0 0,1 4,5A1,1 0 0,1 5,4",
        )
    }
    val photo by lazy { strokeIcon("photo", "M4,5H20V19H4Z M4,15L9,10L14,15L16,13L20,17") }
    val folder by lazy { strokeIcon("folder", "M3,6H9L11,8H21V19H3Z") }
    val monitor by lazy { strokeIcon("monitor", "M3,4H21V16H3Z M8,20H16 M12,16V20") }
    val plus by lazy { strokeIcon("plus", "M12,5V19 M5,12H19") }
    val bolt by lazy { strokeIcon("bolt", "M13,3L5,14H11L10,21L18,10H12Z") }
    val wifi by lazy {
        strokeIcon(
            "wifi",
            "M2,9A15,15 0 0,1 22,9 M5,12.5A10,10 0 0,1 19,12.5 M8.5,16A5,5 0 0,1 15.5,16 M12,19.5H12.01",
        )
    }
    val moon by lazy { strokeIcon("moon", "M20,14A8,8 0 0,1 10,4A8,8 0 1,0 20,14") }
    val info by lazy { strokeIcon("info", "M12,3A9,9 0 1,0 12,21A9,9 0 1,0 12,3 M12,11V16 M12,8H12.01") }
    val keyboard by lazy { strokeIcon("keyboard", "M3,6H21V18H3Z M7,10H7.01 M11,10H11.01 M15,10H15.01 M7,14H17") }
    val volume by lazy { strokeIcon("volume", "M4,9H8L13,5V19L8,15H4Z M17,9A4,4 0 0,1 17,15") }
    val volumeOff by lazy { strokeIcon("volumeOff", "M4,9H8L13,5V19L8,15H4Z M17,9L21,13 M21,9L17,13") }
    val play by lazy { strokeIcon("play", "M7,4L20,12L7,20Z") }
    val pause by lazy { strokeIcon("pause", "M7,4H11V20H7Z M13,4H17V20H13Z") }
    val next by lazy { strokeIcon("next", "M6,5L15,12L6,19Z M18,5V19") }
    val previous by lazy { strokeIcon("previous", "M18,5L9,12L18,19Z M6,5V19") }
    val music by lazy {
        strokeIcon(
            "music",
            "M9,18V5L20,3V16 M9,18A3,3 0 1,1 3,18A3,3 0 1,1 9,18 M20,16A3,3 0 1,1 14,16A3,3 0 1,1 20,16",
        )
    }
    val close by lazy { strokeIcon("close", "M6,6L18,18 M18,6L6,18") }
    val back by lazy { strokeIcon("back", "M15,6L9,12L15,18") }
    val chevronDown by lazy { strokeIcon("chevronDown", "M6,9L12,15L18,9") }
    val chevronRight by lazy { strokeIcon("chevronRight", "M9,6L15,12L9,18") }
    val search by lazy { strokeIcon("search", "M10,4A6,6 0 1,0 10,16A6,6 0 1,0 10,4 M20,20L14.5,14.5") }
    val refresh by lazy { strokeIcon("refresh", "M20,11A8,8 0 1,0 17.7,16.7 M20,4V11H13") }
    val stop by lazy { strokeIcon("stop", "M7,7H17V17H7Z") }
    val screen by lazy { strokeIcon("screen", "M7,3H17V21H7Z M11,18H13") }
    val speaker by lazy { strokeIcon("speaker", "M7,3H17V21H7Z M12,13A2.5,2.5 0 1,0 12,18A2.5,2.5 0 1,0 12,13 M12,7H12.01") }
    val shield by lazy { strokeIcon("shield", "M12,3L20,6V12C20,17 16,20 12,21C8,20 4,17 4,12V6Z") }
    val battery by lazy { strokeIcon("battery", "M3,8H18V16H3Z M21,11V13") }
    val history by lazy { strokeIcon("history", "M4,12A8,8 0 1,0 6.3,6.3 M4,4V9H9 M12,8V12L15,14") }
    val globe by lazy { strokeIcon("globe", "M12,3A9,9 0 1,0 12,21A9,9 0 1,0 12,3 M3,12H21 M12,3C9.5,6 9.5,18 12,21 M12,3C14.5,6 14.5,18 12,21") }
    val palette by lazy {
        strokeIcon(
            "palette",
            "M12,3A9,9 0 1,0 12,21C13.5,21 14,20 13.5,18.5C13,17 14,16 15.5,16H18A3,3 0 0,0 21,13C21,7.5 17,3 12,3 M7.5,11H7.51 M10,7.5H10.01 M14.5,7.5H14.51",
        )
    }
    val trash by lazy { strokeIcon("trash", "M4,7H20 M9,7V4H15V7 M6,7L7,20H17L18,7") }
    val send by lazy { strokeIcon("send", "M4,12L20,4L14,20L11,13Z M11,13L20,4") }
    val person by lazy { strokeIcon("person", "M12,4A4,4 0 1,0 12,12A4,4 0 1,0 12,4 M4,21A8,8 0 0,1 20,21") }
    val sun by lazy {
        strokeIcon(
            "sun",
            "M12,8A4,4 0 1,0 12,16A4,4 0 1,0 12,8 M12,2V4 M12,20V22 M2,12H4 M20,12H22 M4.9,4.9L6.3,6.3 M17.7,17.7L19.1,19.1 M4.9,19.1L6.3,17.7 M17.7,6.3L19.1,4.9",
        )
    }
    val torch by lazy { strokeIcon("torch", "M8,3H16L14,9H10Z M10,9V21H14V9 M12,13V15") }
    val gesture by lazy { strokeIcon("gesture", "M12,3A3,3 0 1,0 12,9A3,3 0 1,0 12,3 M12,9V21 M7,14L12,12L17,14") }
}

/** The G7 mark: its name on a lavender tile. */
@Composable
fun G7Mark(modifier: Modifier = Modifier, large: Boolean = false) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier = modifier
            .then(if (large) Modifier.size(48.dp) else Modifier)
            .clip(RoundedCornerShape(if (large) 12.dp else 6.dp))
            .background(scheme.primary)
            .then(if (large) Modifier else Modifier.padding(horizontal = 6.dp, vertical = 2.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            "G7",
            color = scheme.onPrimary,
            fontWeight = FontWeight.Medium,
            fontSize = if (large) 18.sp else 13.sp,
            maxLines = 1,
        )
    }
}

/** A group's label: small, muted, sentence case. */
@Composable
fun G7SectionLabel(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = modifier
            .padding(start = 12.dp, end = 12.dp, top = 14.dp, bottom = 4.dp)
            .semantics { heading() },
    )
}

/**
 * A row: a tinted icon, a label (and maybe a line under it), a muted note on the
 * right, and whatever [trailing] adds. At least 48 dp tall, with a 10 dp corner
 * when it highlights.
 */
@Composable
fun G7Row(
    label: String,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    tint: Color = Color.Unspecified,
    sublabel: String? = null,
    note: String? = null,
    enabled: Boolean = true,
    highlighted: Boolean = false,
    role: Role = Role.Button,
    onClick: (() -> Unit)? = null,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(shape)
            .then(if (highlighted) Modifier.background(scheme.surfaceContainerHigh) else Modifier)
            .then(
                if (onClick != null) Modifier.clickable(enabled = enabled, role = role, onClick = onClick) else Modifier,
            )
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        if (icon != null) {
            Icon(
                imageVector = icon,
                contentDescription = null,
                tint = if (tint == Color.Unspecified) scheme.onSurfaceVariant else tint,
                modifier = Modifier.size(22.dp),
            )
        }
        Column(Modifier.weight(1f)) {
            Text(
                label,
                style = MaterialTheme.typography.bodyLarge,
                color = scheme.onSurface,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            if (!sublabel.isNullOrEmpty()) {
                Text(
                    sublabel,
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        if (!note.isNullOrEmpty()) {
            Text(
                note,
                style = MaterialTheme.typography.bodyMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        trailing?.invoke(this)
    }
}

/** A row with a switch; the whole row toggles it. */
@Composable
fun G7SwitchRow(
    label: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
    tint: Color = Color.Unspecified,
    sublabel: String? = null,
    enabled: Boolean = true,
    highlighted: Boolean = false,
) {
    G7Row(
        label = label,
        modifier = modifier,
        icon = icon,
        tint = tint,
        sublabel = sublabel,
        enabled = enabled,
        highlighted = highlighted,
        role = Role.Switch,
        onClick = { onCheckedChange(!checked) },
    ) {
        G7Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

@Composable
fun G7Switch(checked: Boolean, onCheckedChange: ((Boolean) -> Unit)?, enabled: Boolean = true) {
    Switch(checked = checked, onCheckedChange = onCheckedChange, enabled = enabled, colors = g7SwitchColors())
}

/** A small action at the end of a row: Allow, Open, Stop. */
@Composable
fun G7RowAction(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    accent: Boolean = false,
    enabled: Boolean = true,
) {
    val scheme = MaterialTheme.colorScheme
    Text(
        label,
        style = MaterialTheme.typography.labelLarge,
        color = if (accent) scheme.onPrimary else scheme.onSurface,
        maxLines = 1,
        modifier = modifier
            .clip(RoundedCornerShape(8.dp))
            .background(if (accent) scheme.primary else scheme.surfaceContainer)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 7.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
    )
}

/** The screen's one primary action: full width, lavender. */
@Composable
fun G7PrimaryButton(label: String, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(scheme.primary)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(14.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, style = MaterialTheme.typography.titleSmall, color = scheme.onPrimary, maxLines = 1)
    }
}

/** A raised, quiet button: clicks, keys, secondary actions. */
@Composable
fun G7RaisedButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    selected: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(if (selected) scheme.surfaceContainerHigh else scheme.surfaceContainer)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 12.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            style = MaterialTheme.typography.labelLarge,
            color = if (selected) scheme.onSurface else scheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A bare icon button in a tint, for toolbars. */
@Composable
fun G7IconButton(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
    enabled: Boolean = true,
    filled: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    val description = contentDescription
    Box(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .then(if (filled) Modifier.background(scheme.surfaceContainer) else Modifier)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { this.contentDescription = description }
            .padding(12.dp)
            .then(if (enabled) Modifier else Modifier.alpha(0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = if (tint == Color.Unspecified) scheme.onSurface else tint,
            modifier = Modifier.size(22.dp),
        )
    }
}

/** A sub-screen's header: Back, the title, and a muted note after it. */
@Composable
fun G7Header(
    title: String,
    onBack: (() -> Unit)?,
    modifier: Modifier = Modifier,
    note: String? = null,
    noteColor: Color = Color.Unspecified,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    Row(
        modifier = modifier.fillMaxWidth().padding(start = 4.dp, end = 8.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (onBack != null) {
            G7IconButton(
                icon = G7Icons.back,
                contentDescription = stringResource(R.string.action_back),
                onClick = onBack,
            )
        } else {
            Box(Modifier.size(8.dp))
        }
        Row(
            modifier = Modifier.weight(1f),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            if (!note.isNullOrEmpty()) {
                Text(
                    note,
                    style = MaterialTheme.typography.labelMedium,
                    color = if (noteColor == Color.Unspecified) MaterialTheme.colorScheme.onSurfaceVariant else noteColor,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        trailing?.invoke(this)
    }
}

/** Text tabs: the picked one sits on the selection fill. */
@Composable
fun <T> G7Tabs(
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = modifier.horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        options.forEach { (value, label) ->
            val isSelected = value == selected
            Text(
                label,
                style = MaterialTheme.typography.labelLarge,
                color = if (isSelected) scheme.onSurface else scheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (isSelected) scheme.surfaceContainerHigh else Color.Transparent)
                    .clickable(role = Role.Tab) { onSelect(value) }
                    .semantics { this.selected = isSelected; role = Role.Tab }
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
    }
}

/**
 * A flat slider: a thin track filled in [tint] up to a small round thumb, like
 * G7's own bars, in place of Material's thick two-part track and bar thumb.
 */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun G7Slider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    valueRange: ClosedFloatingPointRange<Float> = 0f..1f,
    onValueChangeFinished: (() -> Unit)? = null,
    tint: Color = MaterialTheme.colorScheme.primary,
    trackColor: Color = MaterialTheme.colorScheme.surfaceContainerHigh,
    enabled: Boolean = true,
) {
    androidx.compose.material3.Slider(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier,
        enabled = enabled,
        valueRange = valueRange,
        onValueChangeFinished = onValueChangeFinished,
        thumb = {
            Box(Modifier.size(14.dp).clip(RoundedCornerShape(7.dp)).background(tint))
        },
        track = { state ->
            val span = (state.valueRange.endInclusive - state.valueRange.start).takeIf { it > 0f } ?: 1f
            val fraction = ((state.value - state.valueRange.start) / span).coerceIn(0f, 1f)
            androidx.compose.foundation.Canvas(Modifier.fillMaxWidth().heightIn(min = 6.dp)) {
                val height = 6.dp.toPx()
                val top = (size.height - height) / 2f
                val radius = androidx.compose.ui.geometry.CornerRadius(height / 2f, height / 2f)
                drawRoundRect(
                    color = trackColor,
                    topLeft = androidx.compose.ui.geometry.Offset(0f, top),
                    size = androidx.compose.ui.geometry.Size(size.width, height),
                    cornerRadius = radius,
                )
                drawRoundRect(
                    color = tint,
                    topLeft = androidx.compose.ui.geometry.Offset(0f, top),
                    size = androidx.compose.ui.geometry.Size(size.width * fraction, height),
                    cornerRadius = radius,
                )
            }
        },
    )
}

/** A dot in a status color, before a line of status text. */
@Composable
fun G7StatusDot(color: Color, modifier: Modifier = Modifier) {
    Box(modifier.size(6.dp).clip(RoundedCornerShape(3.dp)).background(color))
}
