// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.companion.AssociationInfo
import android.companion.AssociationRequest
import android.companion.BluetoothLeDeviceFilter
import android.companion.CompanionDeviceManager
import android.companion.ObservingDevicePresenceRequest
import android.content.Context
import android.content.IntentSender
import android.os.Build
import java.util.concurrent.Executor
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.resume
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Companion Device Manager (CDM) association with a paired PC: Android lets an
 * associated app run and start its connection from the background. Phones
 * that scan for the PC's rotating beacon ([PcBeaconScan]) find it themselves;
 * older ones are woken by CDM seeing the PC's old beacon (`NCTR\x01` + 8-byte
 * device ID prefix).
 */
object PcCompanion {
    const val MANUFACTURER_ID: Int = 0xFFFF

    /** Association ID to the device ID of the PC it was made for. */
    private const val ASSOCIATIONS = "companion_associations"

    val BEACON_MAGIC: ByteArray = byteArrayOf(
        'N'.code.toByte(),
        'C'.code.toByte(),
        'T'.code.toByte(),
        'R'.code.toByte(),
        1,
    )

    /** z-base-32, as device IDs are written (`core/g7link-protocol/src/id.rs`). */
    private const val ZBASE32 = "ybndrfg8ejkmcpqxot1uwisza345h769"

    /**
     * The bytes of a z-base-32 string, most significant bit first, or null if it
     * has a character outside the alphabet. Trailing bits that don't make a whole
     * byte are dropped, as the encoder pads with them.
     */
    fun decodeZBase32(text: String): ByteArray? {
        val out = java.io.ByteArrayOutputStream()
        var buffer = 0
        var bits = 0
        for (c in text.trim().lowercase()) {
            val value = ZBASE32.indexOf(c)
            if (value < 0) return null
            buffer = (buffer shl 5) or value
            bits += 5
            if (bits >= 8) {
                bits -= 8
                out.write((buffer shr bits) and 0xFF)
            }
        }
        return out.toByteArray()
    }

    /**
     * Builds the manufacturer-specific BLE beacon payload a PC broadcasts
     * (`desktop/app/src/linux/ble_beacon.rs`): the 5-byte `NCTR\x01` magic followed
     * by the first 8 bytes of the PC's 32-byte device ID (13 bytes in all), or
     * just the magic when [deviceId] isn't a valid device ID.
     */
    fun buildBeaconPayload(deviceId: String?): ByteArray {
        val id = deviceId?.let(::decodeZBase32)
        if (id == null || id.size != 32) {
            return BEACON_MAGIC.copyOf()
        }
        return BEACON_MAGIC + id.copyOfRange(0, 8)
    }

    /**
     * Builds the scan filter mask matching [buildBeaconPayload]: 13 `0xFF`
     * bytes when a specific 8-byte device ID prefix is present, or 5 `0xFF`
     * bytes to match any G7 Link PC beacon.
     */
    fun buildBeaconMask(hasDeviceId: Boolean): ByteArray =
        ByteArray(if (hasDeviceId) 13 else 5) { 0xFF.toByte() }

    fun isAssociated(context: Context): Boolean = runCatching {
        val cdm = context.getSystemService(CompanionDeviceManager::class.java) ?: return false
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            cdm.myAssociations.isNotEmpty()
        } else {
            @Suppress("DEPRECATION")
            cdm.associations.isNotEmpty()
        }
    }.getOrDefault(false)

    fun startObservingPresence(context: Context) {
        runCatching {
            val cdm = context.getSystemService(CompanionDeviceManager::class.java) ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
                for (info in cdm.myAssociations) {
                    runCatching {
                        cdm.startObservingDevicePresence(
                            ObservingDevicePresenceRequest.Builder()
                                .setAssociationId(info.id)
                                .build(),
                        )
                    }
                }
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                for (info in cdm.myAssociations) {
                    runCatching {
                        val addr = info.deviceMacAddress?.toString()
                        if (!addr.isNullOrBlank()) {
                            @Suppress("DEPRECATION")
                            cdm.startObservingDevicePresence(addr)
                        }
                    }
                }
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                @Suppress("DEPRECATION")
                for (addr in cdm.associations) {
                    runCatching {
                        cdm.startObservingDevicePresence(addr)
                    }
                }
            }
        }
    }

    private fun associations(context: Context) =
        context.getSharedPreferences(ASSOCIATIONS, Context.MODE_PRIVATE)

    /**
     * Removes the CompanionDeviceManager associations made for a removed PC
     * [pcId] (or all when no PCs remain paired). One made before associations
     * were remembered by PC is found by [pcName].
     */
    fun disassociateForRemovedPc(context: Context, pcId: String?, pcName: String?, noDevicesRemaining: Boolean) {
        runCatching {
            val cdm = context.getSystemService(CompanionDeviceManager::class.java) ?: return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val remembered = associations(context)
                for (info in cdm.myAssociations) {
                    val madeFor = remembered.getString(info.id.toString(), null)
                    val matches = if (madeFor != null) {
                        madeFor == pcId
                    } else {
                        !pcName.isNullOrBlank() && info.displayName?.toString() == pcName
                    }
                    if (noDevicesRemaining || matches) {
                        remembered.edit().remove(info.id.toString()).apply()
                        val addr = info.deviceMacAddress?.toString()
                        if (!addr.isNullOrBlank()) {
                            @Suppress("DEPRECATION")
                            runCatching { cdm.stopObservingDevicePresence(addr) }
                        }
                        runCatching { cdm.disassociate(info.id) }
                    }
                }
            } else if (noDevicesRemaining) {
                @Suppress("DEPRECATION")
                for (addr in cdm.associations) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        runCatching { cdm.stopObservingDevicePresence(addr) }
                    }
                    runCatching { cdm.disassociate(addr) }
                }
            }
        }
    }

    /**
     * The Bluetooth address [pcId]'s rotating beacon comes from right now, from
     * a short scan, or null when it isn't seen within [timeoutMs] (or this phone
     * can't scan, or has no key for it yet).
     */
    suspend fun findPc(context: Context, pcId: String, timeoutMs: Long = 10_000): String? {
        val key = BeaconKeys.all(context)[pcId] ?: return null
        return scanFor(context, timeoutMs) { result ->
            val data = result.scanRecord?.getManufacturerSpecificData(MANUFACTURER_ID)
            val now = System.currentTimeMillis() / 1000
            if (data != null && BeaconMatcher.match(data, mapOf(pcId to key), now) != null) result.device.address else null
        }
    }

    /**
     * [pcId]'s PC while its beacon asks this phone to connect (its `C` tag), from
     * a low-latency scan: the device as scanned, which keeps its address type.
     * Null when it isn't seen within [timeoutMs] (or this phone can't scan, or has
     * no key for it yet).
     */
    suspend fun findPcToConnect(context: Context, pcId: String, timeoutMs: Long = 25_000): BluetoothDevice? {
        val key = BeaconKeys.all(context)[pcId] ?: return null
        return scanFor(context, timeoutMs) { result ->
            val data = result.scanRecord?.getManufacturerSpecificData(MANUFACTURER_ID)
            val now = System.currentTimeMillis() / 1000
            if (data != null && BeaconMatcher.asksToConnect(data, pcId, key, now)) result.device else null
        }
    }

    /**
     * Scans for rotating beacons at low latency until [pick] makes something of a
     * result, for at most [timeoutMs]; null when nothing was picked or this phone
     * can't scan.
     */
    @SuppressLint("MissingPermission") // canScan checks BLUETOOTH_SCAN.
    private suspend fun <T : Any> scanFor(context: Context, timeoutMs: Long, pick: (ScanResult) -> T?): T? {
        if (!PcBeaconScan.canScan(context)) return null
        val scanner = context.getSystemService(BluetoothManager::class.java)?.adapter
            ?.let { runCatching { it.bluetoothLeScanner }.getOrNull() } ?: return null
        return withTimeoutOrNull(timeoutMs) {
            suspendCancellableCoroutine<T?> { cont ->
                val done = AtomicBoolean(false)
                fun finish(found: T?, callback: ScanCallback) {
                    if (!done.compareAndSet(false, true)) return
                    runCatching { scanner.stopScan(callback) }
                    cont.resume(found)
                }
                val callback = object : ScanCallback() {
                    override fun onScanResult(callbackType: Int, result: ScanResult) {
                        pick(result)?.let { finish(it, this) }
                    }

                    override fun onScanFailed(errorCode: Int) = finish(null, this)
                }
                val filter = ScanFilter.Builder()
                    .setManufacturerData(MANUFACTURER_ID, BeaconMatcher.HEADER, byteArrayOf(-1, -1))
                    .build()
                val settings = ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build()
                cont.invokeOnCancellation {
                    if (done.compareAndSet(false, true)) runCatching { scanner.stopScan(callback) }
                }
                runCatching { scanner.startScan(listOf(filter), settings, callback) }
                    .onFailure { finish(null, callback) }
            }
        }
    }

    /**
     * Asks Android to associate G7 Link with the PC [pcDeviceId]: the chooser
     * shows the device at [address] when it was found ([findPc]), or else the
     * one broadcasting that PC's old beacon. Without a valid device ID nothing
     * is asked, so the chooser can't offer someone else's PC.
     */
    fun requestAssociation(
        activity: Activity,
        pcName: String,
        pcDeviceId: String?,
        address: String?,
        onResult: (Boolean) -> Unit = {},
        onLauncherFallback: (IntentSender) -> Unit,
    ) {
        runCatching {
            val cdm = activity.getSystemService(CompanionDeviceManager::class.java) ?: run {
                onResult(false)
                return
            }
            val payload = buildBeaconPayload(pcDeviceId)
            if (pcDeviceId == null || (address == null && payload.size != 13)) {
                onResult(false)
                return
            }
            val scanFilter = if (address != null) {
                ScanFilter.Builder().setDeviceAddress(address).build()
            } else {
                ScanFilter.Builder()
                    .setManufacturerData(MANUFACTURER_ID, payload, buildBeaconMask(true))
                    .build()
            }
            val leFilter = BluetoothLeDeviceFilter.Builder()
                .setScanFilter(scanFilter)
                .build()
            val requestBuilder = AssociationRequest.Builder()
                .addDeviceFilter(leFilter)
                .setSingleDevice(true)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && pcName.isNotBlank()) {
                runCatching { requestBuilder.setDisplayName(pcName) }
            }
            val request = requestBuilder.build()
            val mainExecutor = Executor { runnable -> activity.runOnUiThread(runnable) }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                cdm.associate(
                    request,
                    mainExecutor,
                    object : CompanionDeviceManager.Callback() {
                        override fun onAssociationPending(intentSender: IntentSender) {
                            onLauncherFallback(intentSender)
                        }

                        override fun onAssociationCreated(associationInfo: AssociationInfo) {
                            associations(activity).edit()
                                .putString(associationInfo.id.toString(), pcDeviceId)
                                .apply()
                            startObservingPresence(activity)
                            onResult(true)
                        }

                        override fun onFailure(error: CharSequence?) {
                            onResult(false)
                        }
                    },
                )
            } else {
                cdm.associate(
                    request,
                    object : CompanionDeviceManager.Callback() {
                        @Deprecated("Deprecated in Java")
                        override fun onDeviceFound(chooserLauncher: IntentSender) {
                            onLauncherFallback(chooserLauncher)
                        }

                        override fun onFailure(error: CharSequence?) {
                            onResult(false)
                        }
                    },
                    null,
                )
            }
        }.onFailure {
            onResult(false)
        }
    }
}
