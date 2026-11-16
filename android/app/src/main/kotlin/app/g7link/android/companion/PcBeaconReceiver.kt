// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanResult
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import app.g7link.android.G7LinkApplication

/**
 * Gets what [PcBeaconScan] found, even with the app closed: a beacon whose tag
 * is a paired PC's wakes the app for it ([PcWake]).
 */
class PcBeaconReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_FOUND) return
        if (intent.getIntExtra(BluetoothLeScanner.EXTRA_CALLBACK_TYPE, 0) ==
            android.bluetooth.le.ScanSettings.CALLBACK_TYPE_MATCH_LOST
        ) {
            return
        }
        val results: List<ScanResult> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableArrayListExtra(BluetoothLeScanner.EXTRA_LIST_SCAN_RESULT, ScanResult::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableArrayListExtra(BluetoothLeScanner.EXTRA_LIST_SCAN_RESULT)
        } ?: return
        val keys = BeaconKeys.all(context)
        if (keys.isEmpty()) return
        val now = System.currentTimeMillis() / 1000
        val found = results.asSequence()
            .take(MAX_RESULTS)
            .mapNotNull { result ->
                val data = result.scanRecord?.getManufacturerSpecificData(BeaconMatcher.COMPANY_ID)
                data?.let { BeaconMatcher.match(it, keys, now) }?.let { it to result.device.address }
            }
            .distinctBy { (match, _) -> match.pcId }
            .toList()
        for ((match, _) in found) PcWake.wake(context, match.pcId)
        // A PC that has something for this phone and no shared network can be
        // connected to over Bluetooth for a little while. One at a time.
        val (connect, address) = found.firstOrNull { (match, _) -> match.kind == BeaconMatcher.Kind.CONNECT } ?: return
        val application = context.applicationContext as? G7LinkApplication ?: return
        val pending = goAsync()
        application.core.connectOverBluetooth(connect.pcId, address) { pending.finish() }
    }

    companion object {
        const val ACTION_FOUND = "app.g7link.android.action.PC_BEACON_FOUND"
        /** Results looked at per delivery, however many a batch holds. */
        private const val MAX_RESULTS = 64
    }
}
