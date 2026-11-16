// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Recognises a paired PC's rotating Bluetooth beacon (docs/protocol/bluetooth.md),
 * as `core/g7link-protocol/src/ble.rs` makes it: manufacturer data for company
 * 0xFFFF starting `47 02`, then six-byte slots. A PC's slot is the first six
 * bytes of HMAC-SHA256(beacon key, kind || big-endian window), where the window
 * is Unix seconds / 900. The window either side counts too: the clocks may differ.
 */
object BeaconMatcher {
    const val COMPANY_ID: Int = 0xFFFF
    val HEADER: ByteArray = byteArrayOf(0x47, 0x02)
    const val TAG_LEN = 6
    const val EPOCH_SECS = 900L
    const val MAX_SLOTS = 4

    enum class Kind(val byte: Byte) {
        PRESENCE('P'.code.toByte()),
        CONNECT('C'.code.toByte()),
    }

    data class Match(val pcId: String, val kind: Kind)

    fun epochAt(unixSeconds: Long): Long = unixSeconds / EPOCH_SECS

    fun tag(beaconKey: ByteArray, kind: Kind, epoch: Long): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(beaconKey, "HmacSHA256"))
        mac.update(kind.byte)
        mac.update(java.nio.ByteBuffer.allocate(8).putLong(epoch).array())
        return mac.doFinal().copyOf(TAG_LEN)
    }

    /** The slots of [payload], or null when it isn't a rotating beacon. */
    private fun slots(payload: ByteArray): List<ByteArray>? {
        if (payload.size < HEADER.size || payload[0] != HEADER[0] || payload[1] != HEADER[1]) return null
        val body = payload.size - HEADER.size
        if (body == 0 || body % TAG_LEN != 0 || body / TAG_LEN > MAX_SLOTS) return null
        return (0 until body / TAG_LEN).map { i ->
            payload.copyOfRange(HEADER.size + i * TAG_LEN, HEADER.size + (i + 1) * TAG_LEN)
        }
    }

    /**
     * The PC among [keys] (device ID to beacon key) whose tag [payload] holds,
     * and what it asks; [Kind.CONNECT] wins over [Kind.PRESENCE].
     */
    fun match(payload: ByteArray, keys: Map<String, ByteArray>, unixSeconds: Long): Match? {
        val slots = slots(payload) ?: return null
        val epoch = epochAt(unixSeconds)
        for (kind in listOf(Kind.CONNECT, Kind.PRESENCE)) {
            for ((pcId, key) in keys) {
                for (e in epoch - 1..epoch + 1) {
                    val wanted = tag(key, kind, e)
                    if (slots.any { java.security.MessageDigest.isEqual(it, wanted) }) return Match(pcId, kind)
                }
            }
        }
        return null
    }
}
