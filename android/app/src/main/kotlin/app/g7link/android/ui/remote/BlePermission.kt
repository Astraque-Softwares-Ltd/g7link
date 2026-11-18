// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.Manifest
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.content.BroadcastReceiver
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.Stable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.platform.LocalContext
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.g7link.android.ble.PcBleLink
import app.g7link.android.core.BackgroundAccess

/** How far Android lets this phone use Bluetooth with PCs: the Nearby devices permission. */
enum class NearbyPermission {
    /** Android 11 or older: no Bluetooth link with PCs at all. */
    TooOld,
    Missing,

    /** Denied so that Android won't ask again: only G7 Link's App info can allow it now. */
    Blocked,
    Granted,
}

/**
 * The Nearby devices permission (`BLUETOOTH_CONNECT` and `BLUETOOTH_SCAN`) for
 * a screen: its [state], Android's dialog ([ask]) and, once Android won't show
 * that again, G7 Link's App info ([openAppInfo]).
 */
@Stable
class NearbyDevices internal constructor(
    private val context: Context,
    private val launcher: ActivityResultLauncher<Array<String>>,
    private val granted: MutableState<Boolean>,
    private val blocked: MutableState<Boolean>,
) {
    val state: NearbyPermission
        get() = when {
            Build.VERSION.SDK_INT < Build.VERSION_CODES.S -> NearbyPermission.TooOld
            granted.value -> NearbyPermission.Granted
            blocked.value -> NearbyPermission.Blocked
            else -> NearbyPermission.Missing
        }

    /** Android's dialog for the permission (Android 12 and later). */
    fun ask() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
        runCatching {
            launcher.launch(arrayOf(Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_SCAN))
        }
    }

    fun openAppInfo() {
        runCatching { context.startActivity(BackgroundAccess.appDetailsIntent(context)) }
    }

    /** Looks again: the permission may have been changed in Android's settings. */
    fun refresh() {
        granted.value = PcBleLink.canConnect(context)
        if (granted.value) blocked.value = false
    }
}

/** The Nearby devices permission for this screen; [onResult] hears whether Android's dialog gave it. */
@Composable
fun rememberNearbyDevices(onResult: (granted: Boolean) -> Unit = {}): NearbyDevices {
    val context = LocalContext.current
    val granted = remember { mutableStateOf(PcBleLink.canConnect(context)) }
    val blocked = rememberSaveable { mutableStateOf(false) }
    val latestOnResult by rememberUpdatedState(onResult)
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        val ok = PcBleLink.canConnect(context)
        granted.value = ok
        // Denied, and Android no longer explains why: it won't show its dialog again.
        blocked.value = !ok && !mayAskAgain(context)
        latestOnResult(ok)
    }
    return remember(context, launcher) { NearbyDevices(context, launcher, granted, blocked) }
}

/** Whether Android would still show its dialog for the permission after a denial. */
private fun mayAskAgain(context: Context): Boolean {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return false
    val activity = context.findActivity() ?: return true
    return ActivityCompat.shouldShowRequestPermissionRationale(activity, Manifest.permission.BLUETOOTH_CONNECT)
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/** Whether Bluetooth is on, following its switch (Android 12 and later; false before). */
@Composable
fun rememberBluetoothOn(): State<Boolean> {
    val appContext = LocalContext.current.applicationContext
    val on = remember { mutableStateOf(PcBleLink.bluetoothOn(appContext)) }
    DisposableEffect(appContext) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                on.value = intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR) == BluetoothAdapter.STATE_ON
            }
        }
        ContextCompat.registerReceiver(
            appContext,
            receiver,
            IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        on.value = PcBleLink.bluetoothOn(appContext)
        onDispose { runCatching { appContext.unregisterReceiver(receiver) } }
    }
    return on
}

/** Android's "Turn on Bluetooth?" dialog. It needs `BLUETOOTH_CONNECT`: ask for Nearby devices first. */
fun turnOnBluetoothIntent(): Intent = Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE)
