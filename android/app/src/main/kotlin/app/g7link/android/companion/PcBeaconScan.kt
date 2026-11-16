// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import android.Manifest
import android.annotation.SuppressLint
import android.app.PendingIntent
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import app.g7link.android.ui.Preferences

/**
 * The low-power background scan for paired PCs' rotating beacons
 * (docs/protocol/bluetooth.md). Android runs it with the screen off and the
 * app closed, filtered on the beacon's first bytes, and hands what it finds
 * to [PcBeaconReceiver], which checks each PC's tag. Android 12 and later:
 * older phones scan for the old beacon through CompanionDeviceManager.
 */
object PcBeaconScan {
    private const val TAG = "PcBeaconScan"
    private const val REQUEST_CODE = 0x6762

    /** Whether this phone can scan for the rotating beacon now. */
    fun canScan(context: Context): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_SCAN) ==
            PackageManager.PERMISSION_GRANTED

    private fun pendingIntent(context: Context): PendingIntent {
        val intent = Intent(context, PcBeaconReceiver::class.java).setAction(PcBeaconReceiver.ACTION_FOUND)
        // Mutable: the system adds the scan results to it.
        return PendingIntent.getBroadcast(
            context,
            REQUEST_CODE,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
        )
    }

    /** Starts the scan when there's a PC to look for and it may run; stops it otherwise. */
    @SuppressLint("MissingPermission") // canScan checks BLUETOOTH_SCAN.
    fun sync(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
        val appContext = context.applicationContext ?: context
        val adapter = appContext.getSystemService(BluetoothManager::class.java)?.adapter ?: return
        val scanner = runCatching { adapter.bluetoothLeScanner }.getOrNull() ?: return
        val wanted = canScan(appContext) &&
            Preferences.isBackgroundServiceEnabled(appContext) &&
            BeaconKeys.all(appContext).isNotEmpty()
        val intent = pendingIntent(appContext)
        if (!wanted) {
            runCatching { scanner.stopScan(intent) }
            return
        }
        val filter = ScanFilter.Builder()
            .setManufacturerData(BeaconMatcher.COMPANY_ID, BeaconMatcher.HEADER, byteArrayOf(-1, -1))
            .build()
        val settings = ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_POWER).apply {
            if (adapter.isOffloadedFilteringSupported) {
                setCallbackType(ScanSettings.CALLBACK_TYPE_FIRST_MATCH or ScanSettings.CALLBACK_TYPE_MATCH_LOST)
                setMatchMode(ScanSettings.MATCH_MODE_STICKY)
                setNumOfMatches(ScanSettings.MATCH_NUM_ONE_ADVERTISEMENT)
            } else if (adapter.isOffloadedScanBatchingSupported) {
                setReportDelay(5_000)
            }
        }.build()
        // Started again, it replaces the scan this intent had.
        runCatching { scanner.stopScan(intent) }
        runCatching { scanner.startScan(listOf(filter), settings, intent) }
            .onSuccess { code -> if (code != 0) Log.w(TAG, "the beacon scan didn't start: $code") }
            .onFailure { Log.w(TAG, "can't scan for paired PCs", it) }
    }

    /**
     * Starts the scan again whenever Bluetooth is turned back on (turning it
     * off ends it), until the returned handle is closed.
     */
    fun followAdapter(context: Context): AutoCloseable {
        val appContext = context.applicationContext ?: context
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, -1) == BluetoothAdapter.STATE_ON) sync(context)
            }
        }
        ContextCompat.registerReceiver(
            appContext,
            receiver,
            IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        return AutoCloseable { runCatching { appContext.unregisterReceiver(receiver) } }
    }
}
