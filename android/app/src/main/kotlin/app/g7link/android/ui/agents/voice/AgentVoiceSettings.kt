// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents.voice

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.Settings
import android.speech.SpeechRecognizer
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.content.edit
import app.g7link.android.R
import app.g7link.android.ui.agents.SheetRow
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7Switch

/**
 * Voice on this phone (contract §V8.1, §V8.2): "Read results aloud" and "Also
 * on the phone's speaker", per PC and off until turned on; "Tap to talk", for
 * every reply box.
 */
object AgentVoiceSettings {
    private const val PREFS = "agent_voice"
    private const val READ_ALOUD = "read_aloud."
    private const val SPEAKER = "speaker."
    private const val TAP_TO_TALK = "tap_to_talk"
    private val PER_PC = listOf(READ_ALOUD, SPEAKER)

    fun readAloud(context: Context, pcId: String): Boolean = prefs(context).getBoolean(READ_ALOUD + pcId, false)

    fun setReadAloud(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) putBoolean(READ_ALOUD + pcId, true) else remove(READ_ALOUD + pcId) }
    }

    fun speaker(context: Context, pcId: String): Boolean = prefs(context).getBoolean(SPEAKER + pcId, false)

    fun setSpeaker(context: Context, pcId: String, on: Boolean) {
        prefs(context).edit { if (on) putBoolean(SPEAKER + pcId, true) else remove(SPEAKER + pcId) }
    }

    fun tapToTalk(context: Context): Boolean = prefs(context).getBoolean(TAP_TO_TALK, false)

    fun setTapToTalk(context: Context, on: Boolean) {
        prefs(context).edit { if (on) putBoolean(TAP_TO_TALK, true) else remove(TAP_TO_TALK) }
    }

    /** The PC was unpaired: paired again, it starts from the defaults. */
    fun forgetPc(context: Context, pcId: String) {
        prefs(context).edit { PER_PC.forEach { remove(it + pcId) } }
    }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Whether this phone has an on-device recogniser (Android 13 and later); never the default, online one. */
    fun micAvailable(context: Context): Boolean = AgentVoice.micShown(
        Build.VERSION.SDK_INT,
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            runCatching { SpeechRecognizer.isOnDeviceRecognitionAvailable(context) }.getOrDefault(false),
    )

    /** The system's speech settings, where an offline speech pack is added; Settings when there's none. */
    fun openSpeechSettings(context: Context) {
        val tries = listOf(Intent(Settings.ACTION_VOICE_INPUT_SETTINGS), Intent(Settings.ACTION_SETTINGS))
        for (intent in tries) {
            try {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return
            } catch (_: ActivityNotFoundException) {
                continue
            }
        }
    }
}

/**
 * The Voice part of a PC's agent settings (phase 5 boards 05d, 06a): why
 * there's no mic, and a link to add the offline speech pack; Read results
 * aloud (off without an offline voice); Also on the phone's speaker; Tap to
 * talk.
 */
@Composable
internal fun AgentVoiceSettingsRows(pcId: String, onReadAloudOff: () -> Unit) {
    val context = LocalContext.current
    val scheme = MaterialTheme.colorScheme
    val mic = remember { AgentVoiceSettings.micAvailable(context) }
    val voice by produceState<Boolean?>(null) { value = AgentSpeaker.hasOfflineVoice(context) }
    var readAloud by remember(pcId) { mutableStateOf(AgentVoiceSettings.readAloud(context, pcId)) }
    var speaker by remember(pcId) { mutableStateOf(AgentVoiceSettings.speaker(context, pcId)) }
    var tap by remember { mutableStateOf(AgentVoiceSettings.tapToTalk(context)) }
    G7SectionLabel(stringResource(R.string.agents_settings_voice))
    if (!mic) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Icon(G7Icons.mic, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(20.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(stringResource(R.string.agents_voice_needs_pack), style = MaterialTheme.typography.bodyLarge)
                Text(
                    stringResource(R.string.agents_voice_speech_settings),
                    style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.SemiBold),
                    color = scheme.primary,
                    modifier = Modifier
                        .heightIn(min = 32.dp)
                        .clip(RoundedCornerShape(6.dp))
                        .clickable(role = Role.Button) { AgentVoiceSettings.openSpeechSettings(context) }
                        .padding(vertical = 6.dp),
                )
            }
        }
    }
    // Without an offline voice the switch is off and says why; while it's being checked, as it was.
    val voiceMissing = voice == false
    val canRead = !voiceMissing
    SheetRow(
        label = stringResource(R.string.agents_read_aloud),
        reason = stringResource(if (voiceMissing) R.string.agents_read_aloud_no_voice else R.string.agents_read_aloud_note),
        enabled = canRead,
        onClick = {
            readAloud = !readAloud
            AgentVoiceSettings.setReadAloud(context, pcId, readAloud)
            if (!readAloud) onReadAloudOff()
        },
        trailing = { G7Switch(checked = readAloud && canRead, onCheckedChange = null, enabled = canRead) },
    )
    if (readAloud && canRead) {
        SheetRow(
            label = stringResource(R.string.agents_read_aloud_speaker),
            onClick = {
                speaker = !speaker
                AgentVoiceSettings.setSpeaker(context, pcId, speaker)
            },
            trailing = { G7Switch(checked = speaker, onCheckedChange = null) },
        )
    }
    if (mic) {
        SheetRow(
            label = stringResource(R.string.agents_voice_tap_to_talk),
            onClick = {
                tap = !tap
                AgentVoiceSettings.setTapToTalk(context, tap)
            },
            trailing = { G7Switch(checked = tap, onCheckedChange = null) },
        )
    }
}
