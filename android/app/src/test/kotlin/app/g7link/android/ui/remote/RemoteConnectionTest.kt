// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import app.g7link.android.core.BleSetup
import app.g7link.android.core.CoreState
import app.g7link.android.core.Device
import app.g7link.core.AwayReach
import app.g7link.core.DeviceInfo
import app.g7link.core.DeviceKind
import app.g7link.core.Event
import app.g7link.core.Link
import app.g7link.core.PairedDevice
import app.g7link.core.RemotePath
import app.g7link.core.RemoteRoute
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteConnectionTest {
    private val wifi = Link.Online(relayed = false, usb = false, rttMs = 12u)

    /** A PC that takes the touchpad over Bluetooth, on Wi-Fi and linked, both paths measured. */
    private fun pc(
        route: RemoteRoute = RemoteRoute.AUTO,
        link: Link = wifi,
        active: RemotePath? = RemotePath.SESSION,
        sessionRttMs: UInt? = 9u,
        bleRttMs: UInt? = 34u,
        bleRemote: Boolean = true,
        bleLink: Boolean = true,
        bleLinked: Boolean = true,
        bleSetup: BleSetup? = null,
    ) = Device(
        id = "pc",
        name = "guchu",
        kind = DeviceKind.DESKTOP,
        model = null,
        pairedAt = 0L,
        link = link,
        bleLink = bleLink,
        bleLinked = bleLinked,
        route = route,
        activePath = active,
        sessionRttMs = sessionRttMs,
        bleRttMs = bleRttMs,
        bleRemote = bleRemote,
        bleSetup = bleSetup,
    )

    @Test
    fun `the header names the path in use and its round trip`() {
        assertEquals(RemoteNote.Path(PathName.WiFi, 9, auto = true), remoteNote(pc(), RemoteMode.Touchpad))
        assertEquals(
            RemoteNote.Path(PathName.Bluetooth, 34, auto = true),
            remoteNote(pc(active = RemotePath.BLE), RemoteMode.AirMouse),
        )
        assertEquals(
            "over USB",
            RemoteNote.Path(PathName.Usb, 1, auto = true),
            remoteNote(pc(link = Link.Online(relayed = false, usb = true, rttMs = 1u), sessionRttMs = 1u), RemoteMode.Presentation),
        )
        assertEquals(
            "relayed while away",
            RemoteNote.Path(PathName.Away, 80),
            remoteNote(
                pc(route = RemoteRoute.NETWORK, link = Link.Online(relayed = true, usb = false, rttMs = 80u), sessionRttMs = 80u),
                RemoteMode.Touchpad,
            ),
        )
        assertEquals(
            "the session's own round trip until the touchpad measures one",
            RemoteNote.Path(PathName.WiFi, 12),
            remoteNote(pc(route = RemoteRoute.NETWORK, sessionRttMs = null), RemoteMode.Touchpad),
        )
        assertEquals(
            "no round trip yet",
            RemoteNote.Path(PathName.Bluetooth, null),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, active = RemotePath.BLE, bleRttMs = null), RemoteMode.Touchpad),
        )
    }

    @Test
    fun `a picked path that is down says what input takes instead`() {
        assertEquals(
            RemoteNote.Path(PathName.Bluetooth, 34, warn = true),
            remoteNote(pc(route = RemoteRoute.NETWORK, link = Link.Offline(null), active = RemotePath.BLE), RemoteMode.Touchpad),
        )
        assertEquals(
            RemoteNote.BluetoothComing(PathName.WiFi),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, bleLinked = false), RemoteMode.Touchpad),
        )
        assertEquals(
            "still trying after a miss",
            RemoteNote.BluetoothComing(PathName.WiFi),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, bleLinked = false, bleSetup = BleSetup.OutOfRange), RemoteMode.Touchpad),
        )
        assertEquals(
            "still trying after the PC was found but didn't link",
            RemoteNote.BluetoothComing(PathName.WiFi),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, bleLinked = false, bleSetup = BleSetup.CouldntConnect), RemoteMode.Touchpad),
        )
        assertEquals(
            "the PC opens a window again later",
            RemoteNote.BluetoothComing(PathName.WiFi),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, bleLinked = false, bleSetup = BleSetup.Later), RemoteMode.Touchpad),
        )
        assertEquals(
            "nothing is connecting once the PC said no",
            RemoteNote.Path(PathName.WiFi, 9, warn = true),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, bleLinked = false, bleSetup = BleSetup.Declined), RemoteMode.Touchpad),
        )
        assertEquals(
            RemoteNote.Path(PathName.Bluetooth, 34),
            remoteNote(pc(route = RemoteRoute.BLUETOOTH, active = RemotePath.BLE), RemoteMode.Touchpad),
        )
    }

    @Test
    fun `the header is as before for an older pc, with no path, and on screen and media`() {
        assertEquals(RemoteNote.AsHome, remoteNote(pc(bleRemote = false), RemoteMode.Touchpad))
        assertEquals(
            "an older PC's touchpad with no network",
            RemoteNote.OverBluetooth,
            remoteNote(pc(bleRemote = false, link = Link.Offline(null), active = RemotePath.BLE), RemoteMode.Touchpad),
        )
        assertEquals(RemoteNote.AsHome, remoteNote(pc(active = null), RemoteMode.Touchpad))
        assertEquals(RemoteNote.AsHome, remoteNote(pc(active = RemotePath.BLE), RemoteMode.Screen))
        assertEquals(RemoteNote.AsHome, remoteNote(pc(), RemoteMode.Media))

        assertTrue(opensConnectionSheet(pc(), RemoteMode.Touchpad))
        assertTrue(opensConnectionSheet(pc(), RemoteMode.AirMouse))
        assertTrue(opensConnectionSheet(pc(), RemoteMode.Presentation))
        assertFalse(opensConnectionSheet(pc(), RemoteMode.Screen))
        assertFalse(opensConnectionSheet(pc(), RemoteMode.Media))
        assertFalse("an older PC", opensConnectionSheet(pc(bleRemote = false), RemoteMode.Touchpad))
    }

    @Test
    fun `what this phone lacks comes before the link and the pc`() {
        for (setup in listOf(null, BleSetup.Linked, BleSetup.Declined)) {
            for (linked in listOf(false, true)) {
                fun row(permission: NearbyPermission, on: Boolean = true) =
                    bluetoothRow(RemoteRoute.BLUETOOTH, permission, on, setup, linked, 34)
                assertEquals(RowStatus(RowText.NeedsAndroid12, enabled = false), row(NearbyPermission.TooOld))
                assertEquals(RowStatus(RowText.NeedsPermission, RowAction.Allow), row(NearbyPermission.Missing))
                assertEquals(RowStatus(RowText.NeedsPermission, RowAction.AppInfo), row(NearbyPermission.Blocked))
                assertEquals(RowStatus(RowText.BluetoothOff, RowAction.TurnOn), row(NearbyPermission.Granted, on = false))
            }
        }
    }

    @Test
    fun `a link shows its round trip, whatever the setup said before`() {
        fun row(setup: BleSetup?, rttMs: Int?) =
            bluetoothRow(RemoteRoute.AUTO, NearbyPermission.Granted, true, setup, linked = true, rttMs = rttMs)
        assertEquals(RowStatus(RowText.Rtt(34)), row(BleSetup.Linked, 34))
        assertEquals("before the first ping", RowStatus(RowText.Connected), row(BleSetup.Linked, null))
        assertEquals(RowStatus(RowText.Rtt(34)), row(BleSetup.OutOfRange, 34))
        assertEquals(RowStatus(RowText.Rtt(34)), row(null, 34))
    }

    @Test
    fun `the bring-up's steps and the pc's answers`() {
        fun row(setup: BleSetup?, route: RemoteRoute = RemoteRoute.AUTO) =
            bluetoothRow(route, NearbyPermission.Granted, true, setup, linked = false, rttMs = null)
        assertEquals(RowStatus(RowText.Connecting, busy = true), row(BleSetup.Starting))
        assertEquals(RowStatus(RowText.Connecting, busy = true), row(BleSetup.Connecting))
        assertEquals(RowStatus(RowText.WaitingForPc, busy = true), row(BleSetup.Asking))
        assertEquals(RowStatus(RowText.AllowOnPc, RowAction.Ask), row(BleSetup.NotAllowed))
        assertEquals(RowStatus(RowText.PcSaidNo, RowAction.AskAgain), row(BleSetup.Declined))
        assertEquals(RowStatus(RowText.PcHasNoBluetooth, enabled = false), row(BleSetup.Unavailable))
        assertEquals(RowStatus(RowText.PcBusy, RowAction.TryAgain), row(BleSetup.Busy))
        assertEquals(RowStatus(RowText.OutOfRange, RowAction.TryAgain), row(BleSetup.OutOfRange))
        assertEquals(RowStatus(RowText.CouldntConnect, RowAction.TryAgain), row(BleSetup.CouldntConnect))
        assertEquals(RowStatus(RowText.Later, RowAction.TryAgain), row(BleSetup.Later))
        assertEquals(RowStatus(RowText.ConnectOverNetworkFirst), row(BleSetup.NoPath))
        assertEquals(RowStatus(RowText.Failed, RowAction.TryAgain), row(BleSetup.Failed))
        assertEquals("not tried yet", RowStatus(RowText.NotConnected), row(null))
        assertEquals("linked before, gone since", RowStatus(RowText.NotConnected), row(BleSetup.Linked))
        assertEquals(
            "picked, and gone since",
            RowStatus(RowText.NotConnected, RowAction.TryAgain),
            row(BleSetup.Linked, RemoteRoute.BLUETOOTH),
        )
    }

    @Test
    fun `auto names the path it uses once both are measured`() {
        fun auto(device: Device) = autoRow(device, NearbyPermission.Granted, bluetoothOn = true)
        assertEquals(RowStatus(RowText.Using(PathName.WiFi, 9)), auto(pc()))
        assertEquals(RowStatus(RowText.Using(PathName.Bluetooth, 34)), auto(pc(active = RemotePath.BLE)))
        assertEquals("only one measured", RowStatus(RowText.PicksFaster), auto(pc(bleRttMs = null, bleLinked = false)))
        assertEquals("neither path up", RowStatus(RowText.PicksFaster), auto(pc(active = null)))
        assertEquals("not picked", RowStatus(RowText.PicksFaster), auto(pc(route = RemoteRoute.NETWORK)))
    }

    @Test
    fun `auto says when bluetooth needs setting up, while it's picked`() {
        val setUp = RowStatus(RowText.NeedsSetUp, RowAction.SetUp)
        val notLinked = pc(bleRttMs = null, bleLinked = false)
        assertEquals(setUp, autoRow(notLinked, NearbyPermission.Missing, bluetoothOn = true))
        assertEquals(setUp, autoRow(notLinked, NearbyPermission.Blocked, bluetoothOn = true))
        assertEquals(setUp, autoRow(notLinked, NearbyPermission.Granted, bluetoothOn = false))
        assertEquals("this phone's switch", setUp, autoRow(notLinked.copy(bleLink = false), NearbyPermission.Granted, true))
        assertEquals("the PC said no", setUp, autoRow(notLinked.copy(bleSetup = BleSetup.Declined), NearbyPermission.Granted, true))
        assertEquals(setUp, autoRow(notLinked.copy(bleSetup = BleSetup.NotAllowed), NearbyPermission.Granted, true))

        val nothingToDo = RowStatus(RowText.PicksFaster)
        assertEquals("Android 11", nothingToDo, autoRow(notLinked, NearbyPermission.TooOld, bluetoothOn = false))
        assertEquals(
            "a PC with no Bluetooth",
            nothingToDo,
            autoRow(notLinked.copy(bleSetup = BleSetup.Unavailable), NearbyPermission.Granted, true),
        )
        assertEquals(
            "set up, and out of range",
            nothingToDo,
            autoRow(notLinked.copy(bleSetup = BleSetup.OutOfRange), NearbyPermission.Granted, true),
        )
        assertEquals("on Network", nothingToDo, autoRow(notLinked.copy(route = RemoteRoute.NETWORK), NearbyPermission.Missing, true))
    }

    @Test
    fun `the network row is named after the network's path`() {
        val rows = connectionRows(pc(), NearbyPermission.Granted, bluetoothOn = true)
        assertEquals(PathName.WiFi, rows.networkPath)
        assertEquals(RowStatus(RowText.Rtt(9)), rows.network)
        assertEquals(RowStatus(RowText.Rtt(34)), rows.bluetooth)

        val usb = connectionRows(
            pc(link = Link.Online(relayed = false, usb = true, rttMs = 0u), sessionRttMs = null),
            NearbyPermission.Granted,
            bluetoothOn = true,
        )
        assertEquals(PathName.Usb, usb.networkPath)
        assertEquals("connected, no round trip yet", RowStatus(RowText.Connected), usb.network)

        val offline = connectionRows(pc(link = Link.Offline(null), active = RemotePath.BLE), NearbyPermission.Granted, true)
        assertNull("called Network", offline.networkPath)
        assertEquals(RowStatus(RowText.NotConnected), offline.network)
    }

    @Test
    fun `a direct connection over Tailscale is named Tailscale, not Wi-Fi`() {
        val tailnet = Link.Online(relayed = false, usb = false, rttMs = 120u, tailnet = true)
        assertEquals(PathName.Tailscale, networkPath(tailnet))
        assertEquals(
            "the header's note",
            RemoteNote.Path(PathName.Tailscale, 120, auto = true),
            remoteNote(pc(link = tailnet, sessionRttMs = 120u), RemoteMode.Touchpad),
        )
        val rows = connectionRows(pc(link = tailnet, sessionRttMs = 120u), NearbyPermission.Granted, bluetoothOn = true)
        assertEquals("the network row reads “Tailscale · 120 ms”", PathName.Tailscale, rows.networkPath)
        assertEquals(RowStatus(RowText.Rtt(120)), rows.network)
        assertEquals(RowStatus(RowText.Using(PathName.Tailscale, 120)), autoRow(pc(link = tailnet, sessionRttMs = 120u), NearbyPermission.Granted, true))
        assertEquals("relayed first", PathName.Away, networkPath(Link.Online(relayed = true, usb = false, rttMs = 80u, tailnet = true)))
        assertEquals("then USB", PathName.Usb, networkPath(Link.Online(relayed = false, usb = true, rttMs = 1u, tailnet = true)))
        assertEquals(PathName.WiFi, networkPath(wifi))
    }

    @Test
    fun `the Tailscale row shows when Tailscale is up here or the PC is reached from away`() {
        assertTrue(tailscaleRowShown(tailnetUp = true, away = null))
        assertTrue(tailscaleRowShown(tailnetUp = true, away = AwayReach.HOME_ONLY))
        assertTrue(tailscaleRowShown(tailnetUp = false, away = AwayReach.TAILSCALE_OFF))
        assertTrue(tailscaleRowShown(tailnetUp = false, away = AwayReach.MOBILE_DATA))
        assertTrue(tailscaleRowShown(tailnetUp = false, away = AwayReach.TAILSCALE))
        assertFalse(tailscaleRowShown(tailnetUp = false, away = AwayReach.HOME_ONLY))
        assertFalse("not read yet", tailscaleRowShown(tailnetUp = false, away = null))
    }

    @Test
    fun `a move onto the tailnet without a reconnect reaches the note`() {
        val paired = PairedDevice(
            id = "pc",
            info = DeviceInfo("guchu", DeviceKind.DESKTOP, "linux", "6.12", null, null),
            pairedAt = 0L,
            link = Link.Offline(null),
            canWake = false,
            reconnect = true,
        )
        var state = CoreState()
            .reduce(Event.DeviceAdded(paired))
            .reduce(Event.LinkChanged("pc", Link.Online(relayed = false, usb = false, rttMs = 100u)))
        assertEquals(PathName.WiFi, networkPath(state.device("pc")!!.link))
        // Same round trip, now over Tailscale: still a change.
        state = state.reduce(Event.LinkChanged("pc", Link.Online(relayed = false, usb = false, rttMs = 101u, tailnet = true)))
        assertEquals(PathName.Tailscale, networkPath(state.device("pc")!!.link))
    }

    @Test
    fun `android's steps come before the pc's`() {
        assertEquals(SetupStep.Nothing, nextSetupStep(NearbyPermission.TooOld, bluetoothOn = false))
        assertEquals(SetupStep.AskPermission, nextSetupStep(NearbyPermission.Missing, bluetoothOn = false))
        assertEquals(SetupStep.AskPermission, nextSetupStep(NearbyPermission.Missing, bluetoothOn = true))
        assertEquals(SetupStep.OpenAppInfo, nextSetupStep(NearbyPermission.Blocked, bluetoothOn = true))
        assertEquals(SetupStep.TurnOn, nextSetupStep(NearbyPermission.Granted, bluetoothOn = false))
        assertEquals(SetupStep.Ready, nextSetupStep(NearbyPermission.Granted, bluetoothOn = true))
    }

    @Test
    fun `the link is quick where input may take it`() {
        assertTrue(wantsUrgentLink(pc(), RemoteMode.Touchpad))
        assertTrue(wantsUrgentLink(pc(route = RemoteRoute.BLUETOOTH), RemoteMode.AirMouse))
        assertFalse("on the network, with it up", wantsUrgentLink(pc(route = RemoteRoute.NETWORK), RemoteMode.Touchpad))
        assertTrue(
            "on the network, with it down",
            wantsUrgentLink(pc(route = RemoteRoute.NETWORK, link = Link.Offline(null)), RemoteMode.Presentation),
        )
        assertFalse(wantsUrgentLink(pc(), RemoteMode.Screen))
        assertFalse(wantsUrgentLink(pc(), RemoteMode.Media))
    }

    @Test
    fun `the open box asks again on a new path only`() {
        assertTrue(watchAgain(boxOpen = true, path = RemotePath.BLE, watched = RemotePath.SESSION))
        assertTrue("back after no path", watchAgain(boxOpen = true, path = RemotePath.SESSION, watched = null))
        assertFalse(watchAgain(boxOpen = true, path = RemotePath.SESSION, watched = RemotePath.SESSION))
        assertFalse("closed", watchAgain(boxOpen = false, path = RemotePath.BLE, watched = RemotePath.SESSION))
        assertFalse("no path to ask on", watchAgain(boxOpen = true, path = null, watched = RemotePath.SESSION))
    }
}
