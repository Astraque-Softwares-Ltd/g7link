// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.elevated

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import androidx.core.content.edit
import java.net.Inet4Address
import java.net.Inet6Address
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject

/**
 * A Wi-Fi network as Elevated tells it apart. Android shows a network's name
 * and access point only to apps with location access, which G7 Link doesn't
 * ask for, so it goes by what any app sees: the routers' addresses (an IPv6
 * router's link-local address usually comes from its hardware address) and
 * the DNS domain the network hands out.
 */
data class WifiNetwork(
    val ipv4Gateways: Set<String>,
    val ipv6Gateways: Set<String>,
    val domain: String?,
) {
    /** Whether there's anything to tell it by: a router. */
    val identifiable: Boolean get() = ipv4Gateways.isNotEmpty() || ipv6Gateways.isNotEmpty()

    /** What the user sees for it: the router's address, and the domain when there is one. */
    val address: String
        get() = listOfNotNull(ipv4Gateways.minOrNull() ?: ipv6Gateways.minOrNull(), domain).joinToString(" · ")

    /**
     * Whether [current] is this network: the same domain, and a router in
     * common for each kind of address both have seen. IPv6 often comes up a
     * moment after IPv4, so a kind seen on one side only doesn't decide.
     */
    fun matches(current: WifiNetwork): Boolean {
        if (!identifiable || !current.identifiable || domain != current.domain) return false
        val v4 = ipv4Gateways.isNotEmpty() && current.ipv4Gateways.isNotEmpty()
        val v6 = ipv6Gateways.isNotEmpty() && current.ipv6Gateways.isNotEmpty()
        if (!v4 && !v6) return false
        if (v4 && ipv4Gateways.none(current.ipv4Gateways::contains)) return false
        if (v6 && ipv6Gateways.none(current.ipv6Gateways::contains)) return false
        return true
    }

    fun toJson(): JSONObject = JSONObject()
        .put(V4, JSONArray(ipv4Gateways.sorted()))
        .put(V6, JSONArray(ipv6Gateways.sorted()))
        .put(DOMAIN, domain ?: "")

    companion object {
        private const val V4 = "v4"
        private const val V6 = "v6"
        private const val DOMAIN = "domain"
        /** Trusted networks kept, at most. */
        const val MAX_TRUSTED = 32

        /** A domain as networks hand it out ("home", "fritz.box"), compared without case. */
        fun domainOf(raw: String?): String? =
            raw?.split(' ', ',')?.map { it.trim().lowercase() }?.firstOrNull { it.isNotEmpty() }

        /** A router's address without the interface an IPv6 one names (`%wlan0`). */
        fun gatewayOf(raw: String): String = raw.substringBefore('%').lowercase()

        /** The networks [current] adds to [trusted]: merged into the one it matches, or added. */
        fun trust(trusted: List<WifiNetwork>, current: WifiNetwork): List<WifiNetwork> {
            if (!current.identifiable) return trusted
            val index = trusted.indexOfFirst { it.matches(current) }
            if (index < 0) return (trusted + current).takeLast(MAX_TRUSTED)
            val known = trusted[index]
            val merged = WifiNetwork(
                known.ipv4Gateways + current.ipv4Gateways,
                known.ipv6Gateways + current.ipv6Gateways,
                known.domain,
            )
            return trusted.toMutableList().also { it[index] = merged }
        }

        fun listToJson(networks: List<WifiNetwork>): String =
            JSONArray().also { array -> networks.forEach { array.put(it.toJson()) } }.toString()

        /** The networks saved with [listToJson]; nothing for anything unreadable. */
        fun listFromJson(text: String?): List<WifiNetwork> {
            if (text.isNullOrBlank()) return emptyList()
            val array = runCatching { JSONArray(text) }.getOrNull() ?: return emptyList()
            return (0 until minOf(array.length(), MAX_TRUSTED)).mapNotNull { i ->
                val item = array.optJSONObject(i) ?: return@mapNotNull null
                val strings = { key: String ->
                    val values = item.optJSONArray(key)
                    (0 until (values?.length() ?: 0)).mapNotNull { j -> values?.optString(j)?.takeIf(String::isNotBlank) }.toSet()
                }
                WifiNetwork(strings(V4), strings(V6), domainOf(item.optString(DOMAIN)))
                    .takeIf { it.identifiable }
            }
        }
    }
}

/**
 * The Wi-Fi networks the user trusts with Elevated through Wireless
 * debugging: on any other, G7 Link doesn't turn Wireless debugging on.
 */
object TrustedNetworks {
    private const val PREFS = "elevated"
    private const val KEY = "trusted_networks"

    private val _list = MutableStateFlow<List<WifiNetwork>>(emptyList())
    val list: StateFlow<List<WifiNetwork>> = _list.asStateFlow()
    @Volatile private var loaded = false

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun load(context: Context): List<WifiNetwork> {
        if (!loaded) {
            synchronized(this) {
                if (!loaded) {
                    _list.value = WifiNetwork.listFromJson(prefs(context).getString(KEY, null))
                    loaded = true
                }
            }
        }
        return _list.value
    }

    fun trusted(context: Context): List<WifiNetwork> = load(context)

    fun isTrusted(context: Context, network: WifiNetwork): Boolean = load(context).any { it.matches(network) }

    fun trust(context: Context, network: WifiNetwork) = save(context, WifiNetwork.trust(load(context), network))

    fun remove(context: Context, network: WifiNetwork) = save(context, load(context) - network)

    @Synchronized
    private fun save(context: Context, networks: List<WifiNetwork>) {
        prefs(context).edit { putString(KEY, WifiNetwork.listToJson(networks)) }
        _list.value = networks
    }

    /**
     * The Wi-Fi network the phone is on (not a VPN over it), or null when it
     * isn't on one. It may not be [WifiNetwork.identifiable] yet, right after
     * connecting.
     */
    fun current(context: Context): WifiNetwork? = runCatching {
        val connectivity = context.getSystemService(ConnectivityManager::class.java) ?: return null
        @Suppress("DEPRECATION")
        val wifi = connectivity.allNetworks.firstOrNull { network ->
            connectivity.getNetworkCapabilities(network)?.let {
                it.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) && !it.hasTransport(NetworkCapabilities.TRANSPORT_VPN)
            } == true
        } ?: return null
        val link = connectivity.getLinkProperties(wifi) ?: return WifiNetwork(emptySet(), emptySet(), null)
        val v4 = mutableSetOf<String>()
        val v6 = mutableSetOf<String>()
        for (route in link.routes) {
            if (!route.isDefaultRoute) continue
            val gateway = route.gateway ?: continue
            if (gateway.isAnyLocalAddress) continue
            val address = gateway.hostAddress ?: continue
            when (gateway) {
                is Inet4Address -> v4 += WifiNetwork.gatewayOf(address)
                is Inet6Address -> if (gateway.isLinkLocalAddress) v6 += WifiNetwork.gatewayOf(address)
            }
        }
        WifiNetwork(v4, v6, WifiNetwork.domainOf(link.domains))
    }.getOrNull()
}
