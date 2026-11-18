// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import app.g7link.android.core.BleSetup
import app.g7link.android.core.Device
import app.g7link.core.Link
import app.g7link.core.RemotePath
import app.g7link.core.RemoteRoute

// What the Remote screen says about the touchpad's connection to a PC (Auto, the
// network or Bluetooth): the header's note and the connection sheet's rows,
// worked out from the PC's state. Words come from ConnectionSheet.kt.

/** The tabs whose input can go over Bluetooth: Screen and Media need the network. */
internal val RemoteMode.takesBluetooth: Boolean
    get() = this == RemoteMode.Touchpad || this == RemoteMode.AirMouse || this == RemoteMode.Presentation

/** A way to the PC, as the touchpad names it. */
internal enum class PathName { WiFi, Usb, Away, Bluetooth }

/** The network's way to the PC: relayed while away, USB, or else Wi-Fi. */
internal fun networkPath(link: Link): PathName {
    val online = link as? Link.Online ?: return PathName.WiFi
    return when {
        online.relayed -> PathName.Away
        online.usb -> PathName.Usb
        else -> PathName.WiFi
    }
}

/** The network's round trip in ms: the touchpad's measure, or else the session's own. */
internal fun Device.networkRttMs(): Int? =
    sessionRttMs?.toInt() ?: (link as? Link.Online)?.rttMs?.takeIf { it >= 1u }?.toInt()

internal fun Device.bluetoothRttMs(): Int? = bleRttMs?.toInt()

/** The header's note about the touchpad's connection. */
internal sealed interface RemoteNote {
    /** The network's state, as Home shows it. */
    data object AsHome : RemoteNote

    /** An older PC's touchpad, over Bluetooth because there's no network. */
    data object OverBluetooth : RemoteNote

    /** The path input takes, after "Auto" when Auto chose it; [warn] when the picked one is down. */
    data class Path(val path: PathName, val rttMs: Int?, val auto: Boolean = false, val warn: Boolean = false) : RemoteNote

    /** Bluetooth is picked but isn't up yet: input takes [meanwhile]. */
    data class BluetoothComing(val meanwhile: PathName) : RemoteNote
}

/** Whether the header opens the connection sheet: a PC that takes the touchpad over Bluetooth, on a tab that can. */
internal fun opensConnectionSheet(device: Device, mode: RemoteMode): Boolean = device.bleRemote && mode.takesBluetooth

/** The header's note for [device] on the [mode] tab. */
internal fun remoteNote(device: Device, mode: RemoteMode): RemoteNote {
    if (!device.bleRemote) {
        return if (!device.online && device.bleLinked) RemoteNote.OverBluetooth else RemoteNote.AsHome
    }
    val active = device.activePath
    if (!mode.takesBluetooth || active == null) return RemoteNote.AsHome
    val overBluetooth = active == RemotePath.BLE
    val network = networkPath(device.link)
    return when (device.route) {
        RemoteRoute.AUTO -> if (overBluetooth) {
            RemoteNote.Path(PathName.Bluetooth, device.bluetoothRttMs(), auto = true)
        } else {
            RemoteNote.Path(network, device.networkRttMs(), auto = true)
        }
        RemoteRoute.NETWORK -> if (overBluetooth) {
            RemoteNote.Path(PathName.Bluetooth, device.bluetoothRttMs(), warn = true)
        } else {
            RemoteNote.Path(network, device.networkRttMs())
        }
        RemoteRoute.BLUETOOTH -> when {
            overBluetooth -> RemoteNote.Path(PathName.Bluetooth, device.bluetoothRttMs())
            device.bleSetup in BLUETOOTH_WONT_COME -> RemoteNote.Path(network, device.networkRttMs(), warn = true)
            else -> RemoteNote.BluetoothComing(network)
        }
    }
}

/** Where the bring-up stopped short of a link until the user or the PC acts: nothing is connecting. */
private val BLUETOOTH_WONT_COME: Set<BleSetup?> = setOf(
    BleSetup.NotAllowed,
    BleSetup.Declined,
    BleSetup.Unavailable,
    BleSetup.Busy,
    BleSetup.NoPath,
    BleSetup.Failed,
)

/** What a row of the connection sheet says under its label. */
internal sealed interface RowText {
    data class Rtt(val ms: Int) : RowText
    data object Connected : RowText
    data object NotConnected : RowText
    data object Connecting : RowText
    data class Using(val path: PathName, val rttMs: Int) : RowText
    data object PicksFaster : RowText
    data object NeedsSetUp : RowText
    data object NeedsPermission : RowText
    data object NeedsAndroid12 : RowText
    data object BluetoothOff : RowText
    data object AllowOnPc : RowText
    data object WaitingForPc : RowText
    data object PcSaidNo : RowText
    data object PcHasNoBluetooth : RowText
    data object PcBusy : RowText
    data object OutOfRange : RowText
    data object ConnectOverNetworkFirst : RowText
    data object Failed : RowText
}

/** The button at the end of a row. */
internal enum class RowAction { SetUp, Allow, AppInfo, TurnOn, Ask, AskAgain, TryAgain }

/** A row's line and button; [busy] while something is under way, [enabled] false when it can't be picked. */
internal data class RowStatus(
    val text: RowText,
    val action: RowAction? = null,
    val busy: Boolean = false,
    val enabled: Boolean = true,
)

/** The sheet's three rows; the network row is named after [networkPath], or "Network" when it's null (offline). */
internal data class ConnectionRows(
    val auto: RowStatus,
    val network: RowStatus,
    val networkPath: PathName?,
    val bluetooth: RowStatus,
)

/** The connection sheet's rows for [device], with this phone's [permission] and whether its Bluetooth is on. */
internal fun connectionRows(device: Device, permission: NearbyPermission, bluetoothOn: Boolean): ConnectionRows =
    ConnectionRows(
        auto = autoRow(device, permission, bluetoothOn),
        network = networkRow(device),
        networkPath = if (device.online) networkPath(device.link) else null,
        bluetooth = bluetoothRow(device.route, permission, bluetoothOn, device.bleSetup, device.bleLinked, device.bluetoothRttMs()),
    )

/** Auto names the path it uses once both are measured; while it's picked, it says what Bluetooth still needs. */
internal fun autoRow(device: Device, permission: NearbyPermission, bluetoothOn: Boolean): RowStatus {
    if (device.route != RemoteRoute.AUTO) return RowStatus(RowText.PicksFaster)
    if (bluetoothNeedsSetUp(device, permission, bluetoothOn)) return RowStatus(RowText.NeedsSetUp, RowAction.SetUp)
    val active = device.activePath
    val network = device.networkRttMs()
    val bluetooth = device.bluetoothRttMs()
    if (active == null || network == null || bluetooth == null) return RowStatus(RowText.PicksFaster)
    return if (active == RemotePath.BLE) {
        RowStatus(RowText.Using(PathName.Bluetooth, bluetooth))
    } else {
        RowStatus(RowText.Using(networkPath(device.link), network))
    }
}

/** Whether Bluetooth waits on something the user can give: the permission, Bluetooth on, this phone's switch or the PC's yes. */
internal fun bluetoothNeedsSetUp(device: Device, permission: NearbyPermission, bluetoothOn: Boolean): Boolean =
    when (permission) {
        NearbyPermission.TooOld -> false
        NearbyPermission.Missing, NearbyPermission.Blocked -> true
        NearbyPermission.Granted -> !device.bleLinked &&
            (!bluetoothOn || !device.bleLink || device.bleSetup == BleSetup.NotAllowed || device.bleSetup == BleSetup.Declined)
    }

internal fun networkRow(device: Device): RowStatus = when {
    !device.online -> RowStatus(RowText.NotConnected)
    else -> RowStatus(device.networkRttMs()?.let { RowText.Rtt(it) } ?: RowText.Connected)
}

/**
 * The Bluetooth row from the [route], this phone's [permission], whether its
 * Bluetooth is on, how the bring-up went ([setup]) and the link: first what's in
 * the way on this phone, then the link, then what the PC said.
 */
internal fun bluetoothRow(
    route: RemoteRoute,
    permission: NearbyPermission,
    bluetoothOn: Boolean,
    setup: BleSetup?,
    linked: Boolean,
    rttMs: Int?,
): RowStatus = when {
    permission == NearbyPermission.TooOld -> RowStatus(RowText.NeedsAndroid12, enabled = false)
    permission == NearbyPermission.Missing -> RowStatus(RowText.NeedsPermission, RowAction.Allow)
    permission == NearbyPermission.Blocked -> RowStatus(RowText.NeedsPermission, RowAction.AppInfo)
    !bluetoothOn -> RowStatus(RowText.BluetoothOff, RowAction.TurnOn)
    linked -> RowStatus(rttMs?.let { RowText.Rtt(it) } ?: RowText.Connected)
    else -> when (setup) {
        // Linked before, and the link has gone since.
        null, BleSetup.Linked ->
            RowStatus(RowText.NotConnected, RowAction.TryAgain.takeIf { route == RemoteRoute.BLUETOOTH })
        BleSetup.Starting, BleSetup.Connecting -> RowStatus(RowText.Connecting, busy = true)
        BleSetup.Asking -> RowStatus(RowText.WaitingForPc, busy = true)
        BleSetup.NotAllowed -> RowStatus(RowText.AllowOnPc, RowAction.Ask)
        BleSetup.Declined -> RowStatus(RowText.PcSaidNo, RowAction.AskAgain)
        BleSetup.Unavailable -> RowStatus(RowText.PcHasNoBluetooth, enabled = false)
        BleSetup.Busy -> RowStatus(RowText.PcBusy, RowAction.TryAgain)
        BleSetup.OutOfRange -> RowStatus(RowText.OutOfRange, RowAction.TryAgain)
        BleSetup.NoPath -> RowStatus(RowText.ConnectOverNetworkFirst)
        BleSetup.Failed -> RowStatus(RowText.Failed, RowAction.TryAgain)
    }
}

/** What bringing up Bluetooth needs next on this phone, before the PC is asked. */
internal enum class SetupStep { Nothing, AskPermission, OpenAppInfo, TurnOn, Ready }

internal fun nextSetupStep(permission: NearbyPermission, bluetoothOn: Boolean): SetupStep = when (permission) {
    NearbyPermission.TooOld -> SetupStep.Nothing
    NearbyPermission.Missing -> SetupStep.AskPermission
    NearbyPermission.Blocked -> SetupStep.OpenAppInfo
    NearbyPermission.Granted -> if (bluetoothOn) SetupStep.Ready else SetupStep.TurnOn
}

/** Whether the link should be quick now: a tab that takes Bluetooth shows, and input may go over it. */
internal fun wantsUrgentLink(device: Device, mode: RemoteMode): Boolean =
    mode.takesBluetooth && (device.route != RemoteRoute.NETWORK || !device.online)

/**
 * Whether the open typing box asks for the PC's field again: the PC sends it on
 * the path the last ask came on, so a new path needs a new ask.
 */
internal fun watchAgain(boxOpen: Boolean, path: RemotePath?, watched: RemotePath?): Boolean =
    boxOpen && path != null && path != watched
