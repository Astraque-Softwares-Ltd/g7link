// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.annotation.DrawableRes
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.InlineTextContent
import androidx.compose.foundation.text.appendInlineContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.unit.dp
import app.g7link.android.R

/**
 * An agent's own logo, shown just before its name so Claude and Codex tell
 * apart at a glance: Anthropic's Claude Spark in its Clay, and OpenAI's
 * Blossom, white on a dark theme and black on a light one. Both are the
 * official files, unaltered (plans/brand); they are never tinted.
 */
internal enum class AgentMark(@DrawableRes private val onLight: Int, @DrawableRes private val onDark: Int) {
    CLAUDE(R.drawable.ic_agent_claude, R.drawable.ic_agent_claude),
    OPENAI(R.drawable.ic_agent_openai_black, R.drawable.ic_agent_openai_white),
    ;

    /** The file to draw on a dark or a light theme. */
    @DrawableRes
    fun drawable(dark: Boolean): Int = if (dark) onDark else onLight

    companion object {
        /** The mark for an agent id from the catalog ("claude", "codex"); any other agent has none and keeps its name alone. */
        fun of(agentId: String): AgentMark? = when (agentId) {
            "claude" -> CLAUDE
            "codex" -> OPENAI
            else -> null
        }
    }
}

/** Where a mark sits in text that names its agent ([appendAgentMark]), drawn by [agentMarkContent]. */
private const val MARK_ID = "agent_mark"

private val MARK_SIZE = 14.dp
private val MARK_GAP = 4.dp

/** Puts the agent's mark here, just before its name. A no-break space stands in for it, so it stays with the name and reads as nothing. */
internal fun AnnotatedString.Builder.appendAgentMark() = appendInlineContent(MARK_ID, " ")

/** [text], which starts with the agent's name, with [mark] before it when there is one. */
internal fun markedText(text: String, mark: AgentMark?): AnnotatedString = buildAnnotatedString {
    if (mark != null) appendAgentMark()
    append(text)
}

/** What draws [mark] where [appendAgentMark] put it, untinted, in the file for the theme on screen; nothing without a mark. */
@Composable
internal fun agentMarkContent(mark: AgentMark?): Map<String, InlineTextContent> {
    if (mark == null) return emptyMap()
    val res = mark.drawable(dark = MaterialTheme.colorScheme.surface.luminance() < 0.5f)
    val density = LocalDensity.current
    return remember(res, density) {
        val placeholder = with(density) { Placeholder((MARK_SIZE + MARK_GAP).toSp(), MARK_SIZE.toSp(), PlaceholderVerticalAlign.TextCenter) }
        mapOf(
            MARK_ID to InlineTextContent(placeholder) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.CenterStart) {
                    Image(painterResource(res), contentDescription = null, modifier = Modifier.size(MARK_SIZE))
                }
            },
        )
    }
}
