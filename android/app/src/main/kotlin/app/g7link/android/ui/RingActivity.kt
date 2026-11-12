// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui

import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import app.g7link.android.G7LinkApplication
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7LinkTheme
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.g7Tints
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/** The find-device alarm controls, shown once the device is unlocked. */
class RingActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val core = (application as G7LinkApplication).core
        if (core.state.value.ringingPeer == null) {
            finish()
            return
        }
        lifecycleScope.launch {
            core.state.first { it.ringingPeer == null }
            finish()
        }
        val preferences = Preferences(this)
        setContent {
            val appearance by preferences.appearance.collectAsStateWithLifecycle()
            G7LinkTheme(appearance = appearance) {
                Surface(modifier = Modifier.fillMaxSize()) {
                    val state by core.state.collectAsStateWithLifecycle()
                    val from = state.ringingFrom.orEmpty()
                    Column(
                        modifier = Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing).padding(24.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(24.dp, Alignment.CenterVertically),
                    ) {
                        Icon(
                            G7Icons.ring,
                            contentDescription = null,
                            tint = g7Tints().red,
                            modifier = Modifier.size(56.dp),
                        )
                        Text(
                            if (from.isEmpty()) {
                                stringResource(R.string.ringing_title_unknown)
                            } else {
                                stringResource(R.string.ringing_title, from)
                            },
                            style = MaterialTheme.typography.titleLarge,
                            textAlign = TextAlign.Center,
                        )
                        G7RaisedButton(
                            label = stringResource(R.string.action_stop_ringing),
                            onClick = { core.stopRinging() },
                            modifier = Modifier.fillMaxWidth().heightIn(min = 72.dp),
                        )
                    }
                }
            }
        }
    }

    companion object {
        internal fun intent(context: Context) = Intent(context, RingActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
