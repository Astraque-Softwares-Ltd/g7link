// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanResult
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build

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
            .mapNotNull { it.scanRecord?.getManufacturerSpecificData(BeaconMatcher.COMPANY_ID) }
            .mapNotNull { BeaconMatcher.match(it, keys, now) }
            .distinctBy { it.pcId }
            .toList()
        for (match in found) {
            // Connecting over Bluetooth comes with the Bluetooth link; until
            // then, any tag means the PC is here.
            PcWake.wake(context, match.pcId)
        }
    }

    companion object {
        const val ACTION_FOUND = "app.g7link.android.action.PC_BEACON_FOUND"
        /** Results looked at per delivery, however many a batch holds. */
        private const val MAX_RESULTS = 64
    }
}
