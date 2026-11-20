// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.g7Tints

/**
 * A question the agent asks, in the dock over the reply box (contract §Q6.1,
 * the phone board 7F and 7G): "Claude asks", the ask's tag and "1 of 3", its
 * words, its options as a list (checks when several may be picked), "Something
 * else" that uses what's typed in the reply box, Back and Next or Send answers;
 * Dismiss in the menu. A secret question is answered on the PC: the phone says
 * so and offers Dismiss only, and never shows what it asks.
 */
@Composable
internal fun QuestionDockSheet(
    queue: QuestionQueue,
    answers: QuestionAnswers,
    agentName: String,
    pcName: String,
    replying: Boolean,
    collapsed: Boolean,
    onToggle: () -> Unit,
    maxHeight: Dp,
    onChoose: (String) -> Unit,
    onChooseOther: () -> Unit,
    onBack: () -> Unit,
    onNext: () -> Unit,
    onSend: () -> Unit,
    onDismiss: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val question = queue.shown
    key(question.token) {
        val ask = QuestionDock.ask(question, answers)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(max = maxHeight)
                .clip(RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp))
                .background(scheme.surfaceContainer)
                .padding(start = 14.dp, end = 14.dp, bottom = 14.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(
                Modifier.fillMaxWidth().height(18.dp).clickable(role = Role.Button, onClick = onToggle),
                contentAlignment = Alignment.BottomCenter,
            ) {
                Box(Modifier.width(32.dp).height(4.dp).clip(RoundedCornerShape(2.dp)).background(scheme.outline))
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(G7Icons.help, contentDescription = null, tint = tints.amber, modifier = Modifier.size(16.dp))
                Text(
                    stringResource(R.string.agents_question_asks, agentName),
                    style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                    maxLines = 1,
                )
                val header = ask?.header?.takeIf { it.isNotBlank() }
                Text(
                    header?.let { "· $it" }.orEmpty(),
                    style = MaterialTheme.typography.labelMedium,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                val asks = question.asks.size
                if (asks > 1) {
                    Text(
                        stringResource(R.string.agents_dock_count, answers.index + 1, asks),
                        style = MaterialTheme.typography.labelMedium,
                        color = scheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                } else if (queue.total > 1) {
                    Text(
                        stringResource(R.string.agents_dock_count, queue.position, queue.total),
                        style = MaterialTheme.typography.labelMedium,
                        color = scheme.onSurfaceVariant,
                        maxLines = 1,
                    )
                }
                if (!question.secret) DismissMenu(enabled = !replying && !queue.held, onDismiss = onDismiss)
            }
            if (!collapsed) {
                if (question.secret || ask == null) {
                    SecretBody(pcName)
                    G7RaisedButton(
                        label = stringResource(R.string.agents_question_dismiss),
                        onClick = onDismiss,
                        enabled = !replying && !queue.held,
                        modifier = Modifier.fillMaxWidth(),
                    )
                } else {
                    Column(
                        modifier = Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(ask.text, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.padding(bottom = 6.dp))
                        val picked = answers.picks[ask.id].orEmpty()
                        val enabled = !replying && !queue.held
                        for (option in ask.options) {
                            OptionRow(
                                label = option.label,
                                note = option.note,
                                multi = ask.multi,
                                selected = option.label in picked && (ask.multi || ask.id !in answers.otherOn),
                                enabled = enabled,
                                onClick = { onChoose(option.label) },
                            )
                        }
                        if (ask.other) {
                            OptionRow(
                                label = stringResource(R.string.agents_question_other),
                                note = stringResource(R.string.agents_question_other_note),
                                multi = ask.multi,
                                selected = ask.id in answers.otherOn,
                                enabled = enabled,
                                onClick = onChooseOther,
                            )
                        }
                    }
                    val last = QuestionDock.isLast(question, answers)
                    val back = stringResource(R.string.agents_question_back)
                    if (last) {
                        ButtonPair(
                            secondary = back,
                            onSecondary = onBack,
                            secondaryEnabled = QuestionDock.canBack(answers) && !replying,
                            primary = stringResource(R.string.agents_question_send),
                            onPrimary = onSend,
                            primaryEnabled = QuestionDock.canSend(question, answers, queue.held) && !replying,
                        )
                    } else {
                        ButtonPair(
                            secondary = back,
                            onSecondary = onBack,
                            secondaryEnabled = QuestionDock.canBack(answers) && !replying,
                            primary = stringResource(R.string.agents_question_next),
                            onPrimary = onNext,
                            primaryEnabled = QuestionDock.canNext(question, answers, queue.held) && !replying,
                        )
                    }
                }
                if (queue.held) {
                    Text(
                        stringResource(R.string.agents_held),
                        style = MaterialTheme.typography.bodySmall,
                        color = tints.amber,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (replying) {
                    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    }
                }
            }
        }
    }
}

/** A secret question: answered on the PC only; the phone never has its words. */
@Composable
private fun SecretBody(pcName: String) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surface)
            .padding(horizontal = 12.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(G7Icons.lock, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        Text(stringResource(R.string.agents_question_answer_on, pcName), style = MaterialTheme.typography.bodyMedium)
    }
}

/** The menu at the dock's corner: Dismiss (the agent is told you didn't answer). */
@Composable
private fun DismissMenu(enabled: Boolean, onDismiss: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        G7IconButton(
            icon = AgentGlyphs.more,
            contentDescription = stringResource(R.string.agents_more),
            onClick = { open = true },
            enabled = enabled,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.agents_question_dismiss)) },
                onClick = {
                    open = false
                    onDismiss()
                },
            )
        }
    }
}

/** An option: a radio, or a check where several may be picked; its note under it. */
@Composable
private fun OptionRow(label: String, note: String?, multi: Boolean, selected: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (selected) scheme.surfaceContainerHigh else Color.Transparent)
            .clickable(enabled = enabled, role = if (multi) Role.Checkbox else Role.RadioButton, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val edge = if (selected) scheme.primary else scheme.outline
        if (multi) {
            Box(
                Modifier.size(18.dp).clip(RoundedCornerShape(4.dp)).background(if (selected) scheme.primary else Color.Transparent)
                    .border(1.5.dp, edge, RoundedCornerShape(4.dp)),
                contentAlignment = Alignment.Center,
            ) {
                if (selected) Icon(G7Icons.check, contentDescription = null, tint = scheme.onPrimary, modifier = Modifier.size(12.dp))
            }
        } else {
            Box(Modifier.size(18.dp).border(1.5.dp, edge, CircleShape), contentAlignment = Alignment.Center) {
                if (selected) Box(Modifier.size(9.dp).clip(CircleShape).background(scheme.primary))
            }
        }
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
            if (!note.isNullOrBlank()) {
                Text(note, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}
