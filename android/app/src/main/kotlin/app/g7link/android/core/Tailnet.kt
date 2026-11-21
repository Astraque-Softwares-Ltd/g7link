// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import app.g7link.core.isTailnetAddress

/** Tailscale on this phone, as far as Android's own network list shows it. */
object Tailnet {
    /**
     * Whether Tailscale is up: some VPN network has an address in Tailscale's ranges.
     * The VPN transport is what tells it apart: carriers hand out 100.64.0.0/10 on
     * mobile data too.
     */
    fun isUp(context: Context): Boolean = runCatching {
        val connectivity = context.getSystemService(ConnectivityManager::class.java) ?: return@runCatching false
        @Suppress("DEPRECATION")
        val networks = connectivity.allNetworks
        networks.any { network ->
            val vpn = connectivity.getNetworkCapabilities(network)
                ?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true
            vpn && connectivity.getLinkProperties(network)?.linkAddresses.orEmpty().any { link ->
                link.address.hostAddress?.let(::isTailnetAddress) == true
            }
        }
    }.getOrDefault(false)
}
