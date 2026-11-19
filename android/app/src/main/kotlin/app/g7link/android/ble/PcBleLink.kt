// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ble

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat
import app.g7link.core.BleEndpoint
import app.g7link.core.G7LinkNode
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/** How far an attempt at a link with a PC got before it ended. */
enum class LinkStage { Connecting, Discovering, Subscribing, Handshake, Linked }

/** Why an attempt at a link with a PC came to nothing. */
sealed interface LinkFailure {
    /** No scan saw the PC's connect tag. */
    data object NotSeen : LinkFailure

    /** Connecting failed, or the connection dropped; [status] is Android's (-1 when it gave none). */
    data class Connect(val status: Int) : LinkFailure

    /** The device has no G7 Link service, or Android couldn't read its services. */
    data object NoService : LinkFailure

    /** Subscribing to the PC's notifications failed. */
    data object Subscribe : LinkFailure

    /** Writing the handshake to the PC failed. */
    data object Handshake : LinkFailure

    /** No link within the time allowed. */
    data object TimedOut : LinkFailure

    /** One side refused what the other sent (a handshake that didn't check out, say). */
    data object Refused : LinkFailure
}

/** How the last attempt with a PC ended short of a link, and the stage it reached. */
data class LinkOutcome(val failure: LinkFailure, val stage: LinkStage)

/** What a teardown reason at [stage] says went wrong, when the caller didn't say. */
internal fun failureAt(stage: LinkStage, why: String): LinkFailure = when {
    why == "no G7 Link service" -> LinkFailure.NoService
    stage == LinkStage.Connecting -> LinkFailure.Connect(-1)
    stage == LinkStage.Discovering -> if (why.startsWith("services")) LinkFailure.NoService else LinkFailure.Connect(-1)
    stage == LinkStage.Subscribing -> LinkFailure.Subscribe
    else -> LinkFailure.Handshake
}

/**
 * The phone's side of the Bluetooth channel with a paired PC
 * (docs/protocol/bluetooth.md): connects to the PC's GATT service when its
 * beacon asks, and carries the core's [BleEndpoint] over it. Values written to
 * RX and notified on TX are fragments the endpoint makes and puts together.
 * One link per PC; any failure closes it.
 */
object PcBleLink {
    private const val TAG = "PcBleLink"
    private val SERVICE: UUID = UUID.fromString("bcac09a4-522f-43c6-9533-b30bd7428154")
    private val RX: UUID = UUID.fromString("7f2964e4-c2df-4b2e-8425-68d2be4c85ab")
    private val TX: UUID = UUID.fromString("ea1df14d-6860-4376-a3b0-64fdcb79d7f2")
    private val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
    private const val WANTED_MTU = 517
    private const val CONNECT_WITHIN_MS = 10_000L
    private const val HANDSHAKE_WITHIN_MS = 10_000L

    /** A failed attempt with a PC is tried again at most this often. */
    private const val RETRY_GAP_MS = 15_000L

    /** A link goes back to balanced priority this long after the touchpad stops wanting it quick. */
    private const val CALM_AFTER_MS = 5_000L

    private val links = ConcurrentHashMap<String, Link>()
    private val lastAttempt = ConcurrentHashMap<String, Long>()
    private val lastOutcome = ConcurrentHashMap<String, LinkOutcome>()
    private val timeouts = Handler(Looper.getMainLooper())

    /** PCs whose link the touchpad wants quick, and the step back to balanced waiting for each. */
    private val quick: MutableSet<String> = ConcurrentHashMap.newKeySet()
    private val calming = ConcurrentHashMap<String, Runnable>()

    /** Whether this phone may connect to a PC over Bluetooth now (Android 12 and later). */
    fun canConnect(context: Context): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
            listOf(Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_SCAN).all {
                ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
            }

    /** Whether Bluetooth is on (Android 12 and later; false before, where there's no link). */
    @SuppressLint("MissingPermission") // isEnabled needs no permission from Android 12.
    fun bluetoothOn(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return false
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter ?: return false
        return runCatching { adapter.isEnabled }.getOrDefault(false)
    }

    /** Whether a link with [pcId] is being made or is up. */
    fun active(pcId: String): Boolean = links.containsKey(pcId)

    /** Whether a link with [pcId] is up: connected, subscribed and past the handshake. */
    fun linkedTo(pcId: String): Boolean = links[pcId]?.linked == true

    /** How the last attempt with [pcId] ended short of a link; null once one came up, or before any. */
    fun lastOutcome(pcId: String): LinkOutcome? = lastOutcome[pcId]

    /** Connects to the PC [pcId] at [address], unless a link with it is under way. */
    @SuppressLint("MissingPermission") // The caller checks BLUETOOTH_CONNECT.
    fun connect(context: Context, node: G7LinkNode, pcId: String, address: String) {
        if (links.containsKey(pcId) || !mayTry(pcId, userStarted = false)) return
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter ?: return
        val device = runCatching { adapter.getRemoteDevice(address) }.getOrNull() ?: return
        startLink(context, node, pcId, device)
    }

    /**
     * Connects to the PC [pcId] as a scan found it ([device] keeps its address
     * type), unless a link with it is under way. [urgent] is an attempt the user
     * started: it doesn't wait out the gap after a failed one. Whether an
     * attempt is under way now.
     */
    @SuppressLint("MissingPermission") // canConnect checks BLUETOOTH_CONNECT.
    fun connect(context: Context, node: G7LinkNode, pcId: String, device: BluetoothDevice, urgent: Boolean = true): Boolean {
        if (links.containsKey(pcId)) return true
        if (!canConnect(context) || !mayTry(pcId, userStarted = urgent)) return false
        return startLink(context, node, pcId, device)
    }

    /**
     * High connection priority for [pcId]'s link while the touchpad wants it ([on]),
     * back to balanced [CALM_AFTER_MS] after it stops. Kept for a link that comes up
     * later. Called on the main thread.
     */
    fun setUrgent(pcId: String, on: Boolean) {
        calming.remove(pcId)?.let { timeouts.removeCallbacks(it) }
        if (on) {
            quick += pcId
            links[pcId]?.setPriority(high = true)
            return
        }
        val calm = object : Runnable {
            override fun run() {
                if (!calming.remove(pcId, this)) return
                quick -= pcId
                links[pcId]?.setPriority(high = false)
            }
        }
        calming[pcId] = calm
        timeouts.postDelayed(calm, CALM_AFTER_MS)
    }

    /** Whether an attempt with [pcId] may start now; one that may is counted. */
    private fun mayTry(pcId: String, userStarted: Boolean): Boolean {
        val now = android.os.SystemClock.elapsedRealtime()
        val last = lastAttempt[pcId]
        if (!userStarted && last != null && now - last in 0L until RETRY_GAP_MS) return false
        lastAttempt[pcId] = now
        return true
    }

    /** Starts a link with [pcId] at [device]: whether one is under way now. */
    private fun startLink(context: Context, node: G7LinkNode, pcId: String, device: BluetoothDevice): Boolean {
        val endpoint = runCatching { node.bleConnect(pcId) }.getOrElse {
            Log.i(TAG, "won't connect over Bluetooth: ${it.message}")
            return false
        }
        val link = Link(pcId, endpoint)
        if (links.putIfAbsent(pcId, link) != null) {
            runCatching { endpoint.end() }
            runCatching { endpoint.close() }
            return true
        }
        lastOutcome.remove(pcId)
        link.open(context, device)
        return true
    }

    /** The core has something for the link [endpointId]. */
    fun outbound(endpointId: ULong) {
        links.values.firstOrNull { it.id == endpointId }?.pump()
    }

    /** Ends every link (the core is stopping). */
    fun closeAll() {
        links.values.toList().forEach { it.teardown("closing") }
    }

    private class Link(val pcId: String, val endpoint: BleEndpoint) {
        val id: ULong = endpoint.id()
        private var gatt: BluetoothGatt? = null
        private var rx: BluetoothGattCharacteristic? = null
        private var writing = false
        @Volatile private var closed = false
        @Volatile private var stage = LinkStage.Connecting
        /** Subscribed to TX: the link takes priority and PHY requests from here on. */
        @Volatile private var ready = false
        private var priorityHigh = false
        private val queue = GattQueue { why -> teardown(why) }

        /** Up: connected, subscribed and past the handshake. */
        val linked: Boolean
            get() = ready && !closed && runCatching { endpoint.peer() }.getOrNull() != null

        @SuppressLint("MissingPermission")
        fun open(context: Context, device: BluetoothDevice) {
            timeouts.postDelayed(
                {
                    if (!closed && runCatching { endpoint.peer() }.getOrNull() == null) {
                        teardown("not linked in time", LinkFailure.TimedOut)
                    }
                },
                CONNECT_WITHIN_MS + HANDSHAKE_WITHIN_MS,
            )
            gatt = runCatching { device.connectGatt(context, false, callback, BluetoothDevice.TRANSPORT_LE) }
                .getOrNull()
            if (gatt == null) teardown("couldn't connect", LinkFailure.Connect(-1))
        }

        /** Writes the endpoint's next fragment, one at a time. */
        @SuppressLint("MissingPermission")
        @Synchronized
        fun pump() {
            if (closed || writing) return
            val g = gatt ?: return
            val characteristic = rx ?: return
            val fragment = runCatching { endpoint.nextOutbound() }.getOrNull() ?: return
            writing = true
            queue.add("write") {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    g.writeCharacteristic(characteristic, fragment, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT) ==
                        android.bluetooth.BluetoothStatusCodes.SUCCESS
                } else {
                    @Suppress("DEPRECATION")
                    characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
                    @Suppress("DEPRECATION")
                    characteristic.value = fragment
                    @Suppress("DEPRECATION")
                    g.writeCharacteristic(characteristic)
                }
            }
        }

        /** Asks for high or balanced connection priority (best effort: Android may not grant it). */
        @SuppressLint("MissingPermission") // Only a link connect opened, with BLUETOOTH_CONNECT.
        fun setPriority(high: Boolean) {
            val g = gatt ?: return
            synchronized(this) {
                if (!ready || closed || high == priorityHigh) return
                priorityHigh = high
            }
            val wanted = if (high) BluetoothGatt.CONNECTION_PRIORITY_HIGH else BluetoothGatt.CONNECTION_PRIORITY_BALANCED
            queue.add("priority") {
                runCatching { g.requestConnectionPriority(wanted) }
                doneSoon()
            }
        }

        /** Subscribed: 2M PHY is asked for once, and the priority follows the touchpad. */
        @SuppressLint("MissingPermission") // Only a link connect opened, with BLUETOOTH_CONNECT.
        private fun onReady(g: BluetoothGatt) {
            ready = true
            queue.add("phy") {
                runCatching {
                    g.setPreferredPhy(
                        BluetoothDevice.PHY_LE_2M_MASK,
                        BluetoothDevice.PHY_LE_2M_MASK,
                        BluetoothDevice.PHY_OPTION_NO_PREFERRED,
                    )
                }
                doneSoon()
            }
            setPriority(high = pcId in quick)
        }

        /** For a request whose callback doesn't matter: it's done once it's made. */
        private fun doneSoon(): Boolean {
            timeouts.post { queue.complete(true) }
            return true
        }

        /** Ends the link; [failure] says what went wrong, or else it's worked out from the stage reached. */
        @SuppressLint("MissingPermission")
        fun teardown(why: String, failure: LinkFailure? = null) {
            synchronized(this) {
                if (closed) return
                closed = true
            }
            val reached = if (runCatching { endpoint.peer() }.getOrNull() != null) LinkStage.Linked else stage
            if (reached == LinkStage.Linked) {
                Log.i(TAG, "Bluetooth link with a PC ended: $why")
            } else {
                val outcome = LinkOutcome(failure ?: failureAt(reached, why), reached)
                lastOutcome[pcId] = outcome
                Log.i(TAG, "Bluetooth link with a PC ended: $why (at ${reached.name.lowercase()}: ${outcome.failure})")
            }
            links.remove(pcId, this)
            runCatching { endpoint.end() }
            runCatching { endpoint.close() }
            gatt?.let { g ->
                runCatching { g.disconnect() }
                runCatching { g.close() }
            }
            gatt = null
        }

        private val callback = object : BluetoothGattCallback() {
            @SuppressLint("MissingPermission")
            override fun onConnectionStateChange(g: BluetoothGatt, status: Int, newState: Int) {
                if (newState != BluetoothProfile.STATE_CONNECTED || status != BluetoothGatt.GATT_SUCCESS) {
                    teardown("disconnected ($status)", LinkFailure.Connect(status))
                    return
                }
                stage = LinkStage.Discovering
                queue.add("mtu") { g.requestMtu(WANTED_MTU) }
            }

            @SuppressLint("MissingPermission")
            override fun onMtuChanged(g: BluetoothGatt, mtu: Int, status: Int) {
                if (status == BluetoothGatt.GATT_SUCCESS) runCatching { endpoint.setMtu(mtu.toUShort()) }
                queue.complete(true)
                queue.add("services") { g.discoverServices() }
            }

            @SuppressLint("MissingPermission")
            override fun onServicesDiscovered(g: BluetoothGatt, status: Int) {
                val service = g.getService(SERVICE)
                val rxChar = service?.getCharacteristic(RX)
                val txChar = service?.getCharacteristic(TX)
                val cccd = txChar?.getDescriptor(CCCD)
                if (status != BluetoothGatt.GATT_SUCCESS || rxChar == null || txChar == null || cccd == null) {
                    queue.fail("no G7 Link service")
                    return
                }
                queue.complete(true)
                stage = LinkStage.Subscribing
                if (!g.setCharacteristicNotification(txChar, true)) {
                    queue.fail("notifications refused")
                    return
                }
                queue.add("subscribe") {
                    val enable = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        g.writeDescriptor(cccd, enable) == android.bluetooth.BluetoothStatusCodes.SUCCESS
                    } else {
                        @Suppress("DEPRECATION")
                        cccd.value = enable
                        @Suppress("DEPRECATION")
                        g.writeDescriptor(cccd)
                    }
                }
                rx = rxChar
            }

            override fun onDescriptorWrite(g: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) {
                if (status == BluetoothGatt.GATT_SUCCESS) stage = LinkStage.Handshake
                queue.complete(status == BluetoothGatt.GATT_SUCCESS)
                // Subscribed: the handshake's first message goes now.
                pump()
                if (status == BluetoothGatt.GATT_SUCCESS) onReady(g)
            }

            override fun onCharacteristicWrite(
                g: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                synchronized(this@Link) { writing = false }
                queue.complete(status == BluetoothGatt.GATT_SUCCESS)
                pump()
            }

            override fun onCharacteristicChanged(
                g: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) {
                received(characteristic, value)
            }

            @Deprecated("Deprecated in API 33")
            @Suppress("DEPRECATION")
            override fun onCharacteristicChanged(g: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) received(characteristic, characteristic.value ?: return)
            }
        }

        private fun received(characteristic: BluetoothGattCharacteristic, value: ByteArray) {
            if (characteristic.uuid != TX || closed) return
            // The PC ends a link it can't disconnect alone this way (docs/protocol/bluetooth.md §4.2).
            if (value.isEmpty()) {
                teardown("ended by the PC", LinkFailure.Refused)
                return
            }
            runCatching { endpoint.receive(value) }.onFailure {
                teardown("refused: ${it.message}", LinkFailure.Refused)
                return
            }
            pump()
        }
    }
}
