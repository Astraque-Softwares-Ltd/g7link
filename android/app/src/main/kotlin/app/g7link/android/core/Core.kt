// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.wifi.WifiManager
import android.os.Build
import android.os.SystemClock
import android.service.notification.NotificationListenerService
import android.util.Log
import android.util.LruCache
import android.widget.Toast
import android.net.Uri
import app.g7link.android.BuildConfig
import app.g7link.android.audio.PcSpeakerPlayer
import app.g7link.android.ble.LinkFailure
import app.g7link.android.ble.PcBleLink
import app.g7link.android.clipboard.PhoneClipboard
import app.g7link.android.links.LinkNotifications
import app.g7link.android.media.PcMedia
import app.g7link.android.media.PhoneMedia
import app.g7link.android.files.OutgoingFiles
import app.g7link.android.files.ReceivedFiles
import app.g7link.android.files.TransferNotifications
import app.g7link.android.R
import app.g7link.android.notifications.NotificationListener
import app.g7link.android.notifications.PcNotificationPoster
import app.g7link.android.calls.CallCompanion
import app.g7link.android.calls.PhoneCalls
import app.g7link.android.companion.PcCompanion
import app.g7link.android.contacts.PhoneContacts
import app.g7link.android.elevated.Elevated
import app.g7link.android.mirror.AppWindows
import app.g7link.android.mirror.InputService
import app.g7link.android.mirror.MirrorRequests
import app.g7link.android.photos.RecentPhotos
import app.g7link.android.recorder.DeliveryState
import app.g7link.android.recorder.RecordingsStore
import app.g7link.android.recorder.SavedRecording
import app.g7link.android.service.ConnectionService
import app.g7link.android.service.PcAutoStart
import app.g7link.android.sms.PhoneSms
import app.g7link.android.storage.PhoneStorage
import app.g7link.android.mic.MicService
import app.g7link.android.mirror.MirrorService
import app.g7link.android.toggles.PhoneToggles
import app.g7link.android.webcam.WebcamRequests
import app.g7link.android.webcam.WebcamService
import app.g7link.core.AgentExport
import app.g7link.core.AgentMode
import app.g7link.core.AgentPage
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionPage
import app.g7link.core.AgentSessionState
import app.g7link.core.AgentThread
import app.g7link.core.BleUseState
import app.g7link.core.FfiQueueStatus
import app.g7link.core.Event
import app.g7link.core.EventListener
import app.g7link.core.FileToSend
import app.g7link.core.MirrorStream
import app.g7link.core.Link
import app.g7link.core.G7LinkException
import app.g7link.core.G7LinkNode
import app.g7link.core.NodeOptions
import app.g7link.core.Notification
import app.g7link.core.PairingFailure
import app.g7link.core.PowerLevel
import app.g7link.core.RecordingMarker
import app.g7link.core.RelayEdit
import app.g7link.core.RelayEditResult
import app.g7link.core.RelayProbe
import app.g7link.core.RelaySnapshot
import app.g7link.core.RemoteRoute
import app.g7link.core.RemoteStorageEntry
import app.g7link.core.TimelineEntry
import app.g7link.core.TimelineKind
import app.g7link.core.TimelinePage
import app.g7link.core.Transfer
import app.g7link.core.TransferDirection
import app.g7link.core.TransferStatus
import app.g7link.core.initLogging
import app.g7link.android.files.transferTitle
import app.g7link.android.notifications.AgentNotifications
import app.g7link.android.ui.agents.AgentCall
import app.g7link.android.ui.agents.AgentFailure
import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The app's single connection to `g7link-core`. Owns the node, folds
 * its events into [state], feeds it the phone's battery and network
 * changes, and runs commands for the UI.
 */
@androidx.compose.runtime.Stable
class Core(context: Context, private val scope: CoroutineScope) : EventListener {
    private val context = context.applicationContext.also { PhoneClipboard.init(it) }
    private val ringer = Ringer(this.context)
    private val pcNotificationPoster = PcNotificationPoster(this.context)
    private val pcSpeakerPlayer = PcSpeakerPlayer()
    /** What plays on this phone, for PCs (needs notification access). */
    private val phoneMedia = PhoneMedia(this.context) { players ->
        syncActiveMediaColor()
        enqueueNotificationOp { it.mediaChanged(players) }
    }
    private val platform = PhonePlatform(
        this.context,
        ringer,
        phoneMedia,
        onLink = { pc, url -> LinkNotifications.show(this.context, _state.value.nameOf(pc).orEmpty(), url) },
        onMic = { pc ->
            // Asked on Home too, so it's seen with notifications off or while the app is open.
            _state.update { it.copy(micRequestFrom = pc) }
            app.g7link.android.mic.MicRequests.show(this.context, pc, _state.value.nameOf(pc).orEmpty())
            PcAutoStart.tryStart(this.context, PcAutoStart.Kind.MIC, pc)
            true
        },
        onMicStop = { pc -> clearMicRequest(pc) },
        onWatch = { pc, player, title, file ->
            app.g7link.android.media.WatchOffers.show(
                this.context,
                pc,
                _state.value.nameOf(pc).orEmpty(),
                player,
                title,
                file,
            )
        },
        onTask = { pc, task ->
            app.g7link.android.notifications.TaskNotifications.show(
                this.context,
                pc,
                _state.value.nameOf(pc).orEmpty(),
                task,
            )
        },
        calls = { calls },
        contacts = { contacts },
        sms = { sms },
        toggles = { toggles },
        storage = { storage },
        onMirror = { pc, request ->
            MirrorRequests.show(this.context, request, _state.value.nameOf(pc).orEmpty())
        },
        onWebcam = { pc, request ->
            val session = WebcamService.session.value
            if (session.active && session.pcId == pc && WebcamService.hasPermission(this.context)) {
                WebcamService.start(this.context, pc, request.height, request.fps, request.camera)
                true
            } else {
                _state.update { it.copy(webcamRequest = request) }
                WebcamRequests.show(this.context, request, _state.value.nameOf(pc).orEmpty())
                PcAutoStart.tryStart(this.context, PcAutoStart.Kind.WEBCAM, pc)
                true
            }
        },
        appWindows = AppWindows(this.context, scope, open = { pc -> mirrorOpen(pc) }, nameOf = { pc -> _state.value.nameOf(pc).orEmpty() }),
        nameOf = { pc -> _state.value.nameOf(pc).orEmpty() },
        onKeyboardFromPc = { pc, on -> setKeyboardFromPc(pc, on) },
        onLockedInput = { pc, session ->
            scope.launch(Dispatchers.IO) { runCatching { node?.mirrorLocked(pc, session) } }
        },
    )
    private val storage = PhoneStorage(this.context) { path ->
        enqueueNotificationOp { it.storageChanged(path) }
    }
    private val toggles = PhoneToggles(
        this.context,
        onChanged = { state ->
            enqueueNotificationOp { node ->
                runCatching { node.togglesChanged(state) }.onFailure { Log.i(TAG, "toggles weren't reported", it) }
            }
        },
        onCapabilitiesChanged = { refreshNotificationAccess() },
    )
    private val sms = PhoneSms(this.context) {
        enqueueNotificationOp { it.smsChanged(null) }
    }
    private val calls = PhoneCalls(
        this.context,
        onChange = { call ->
            enqueueNotificationOp { node ->
                runCatching { node.callChanged(call) }.onFailure { Log.i(TAG, "a call wasn't reported", it) }
            }
        },
        onLogChanged = {
            enqueueNotificationOp { it.callLogChanged() }
        },
    )
    private val contacts = PhoneContacts(this.context) {
        enqueueNotificationOp { it.contactsChanged() }
    }
    private val photos = RecentPhotos(
        this.context,
        onPhoto = { photo ->
            enqueueNotificationOp { node ->
                runCatching { node.photoTaken(photo) }.onFailure { Log.i(TAG, "a new photo wasn't announced", it) }
            }
        },
        onChanged = {
            enqueueNotificationOp { it.photosChanged() }
        },
    )

    /** What plays on the PCs, in Android's media controls. */
    val pcMedia = PcMedia(
        context = this.context,
        scope = scope,
        onSeedColorChanged = { syncActiveMediaColor() },
        setPcAudio = { pc, volume, muted -> setPcAudio(pc, volume, muted) },
        node = { startJob?.join(); node },
    ) { pc, player, action, position ->
        command { it.mediaCommand(pc, player, action, position) }
    }.also { it.volumeKeys(app.g7link.android.ui.Preferences.isVolumeKeysControlPcEnabled(this.context)) }

    private fun syncActiveMediaColor() {
        val nextColor = phoneMedia.activeSeedColor ?: pcMedia.activeSeedColor
        if (_state.value.activeMediaColor != nextColor) {
            _state.update { if (it.activeMediaColor != nextColor) it.copy(activeMediaColor = nextColor) else it }
        }
        accent.updateMediaColor(nextColor)
    }
    /**
     * Notification changes, in order: a removal must never overtake the post
     * it removes. Runs once the node is up.
     */
    private val notificationOps = Channel<suspend (G7LinkNode) -> Unit>(Channel.UNLIMITED)

    /**
     * Remote input for PCs, sent one after another: a button's release must
     * never overtake its press (or the PC's button would stay down), nor a
     * laser update its "off".
     */
    private val remoteOps = Channel<suspend (G7LinkNode) -> Unit>(Channel.UNLIMITED)

    /**
     * Pointer moves and smooth scrolls (QUIC datagrams), never queued behind
     * [remoteOps]' replies. Each kind keeps at most one op here, summing
     * deltas until it runs.
     */
    private val pointerOps = Channel<suspend (G7LinkNode) -> Unit>(Channel.UNLIMITED)

    private val opsLock = Any()
    @Volatile private var isRunning = false

    private fun enqueueNotificationOp(op: suspend (G7LinkNode) -> Unit) {
        synchronized(opsLock) {
            if (isRunning && node != null) {
                notificationOps.trySend(op)
            }
        }
    }

    private fun enqueueRemoteOp(op: suspend (G7LinkNode) -> Unit) {
        synchronized(opsLock) {
            if (isRunning && node != null) {
                remoteOps.trySend(op)
            }
        }
    }

    private fun enqueuePointerOp(op: suspend (G7LinkNode) -> Unit): Boolean =
        synchronized(opsLock) {
            isRunning && node != null && pointerOps.trySend(op).isSuccess
        }

    private val _state = MutableStateFlow(CoreState())
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)

    /** Screenshots and recordings of a PC's screen, taken from this phone. */
    val pcCapture = PcCapture(
        this.context,
        scope,
        node = {
            startJob?.join()
            node
        },
        nameOf = { pcId -> _state.value.nameOf(pcId)?.ifBlank { null } ?: this.context.getString(R.string.your_pc) },
        say = { message -> _messages.tryEmit(message) },
    )
    private val _queuedNotices = MutableSharedFlow<OfflineQueuedNotice>(extraBufferCapacity = 8)
    private val _clipSuggestions = MutableSharedFlow<ClipSuggestionNotice>(extraBufferCapacity = 4)
    @Volatile private var node: G7LinkNode? = null
    @Volatile private var startJob: Job? = null
    @Volatile private var runJob: Job? = null
    @Volatile private var runScope: CoroutineScope? = null
    @Volatile private var networkChange: Job? = null
    private val lifecycleMutex = Mutex()
    private val startGuard = StartGuard()
    @Volatile private var activeStartSeq = 0
    private val wakeTimers = java.util.concurrent.ConcurrentHashMap<String, Job>()

    @Volatile private var interactive = false
    private val interactiveLock = Any()

    @Volatile private var lastBattery: app.g7link.core.Battery? = null
    @Volatile private var lastSignal: SignalSnapshot = SignalSnapshot()
    @Volatile private var lastReportedBattery: app.g7link.core.Battery? = null

    private fun pushBatteryAndSignal() {
        val base = lastBattery ?: return
        val sig = lastSignal
        val merged = base.copy(
            cellBars = sig.cellBars,
            cellType = sig.cellType,
            wifiBars = sig.wifiBars,
        )
        if (merged != lastReportedBattery) {
            lastReportedBattery = merged
            node?.let { scope.launch { it.updateBattery(merged) } }
        }
    }

    private val battery = BatteryMonitor(this.context) { battery ->
        lastBattery = battery
        pushBatteryAndSignal()
    }
    private val signal = SignalMonitor(this.context) { snapshot ->
        val hadWifi = lastSignal.wifiBars != null
        val hasWifi = snapshot.wifiBars != null
        lastSignal = snapshot
        pushBatteryAndSignal()
        if (hadWifi != hasWifi && (hasWifi || _state.value.awayMode)) {
            node?.let { scope.launch { runCatching { it.networkChanged() } } }
        }
    }
    private val accent = AccentMonitor(this.context) { seed ->
        node?.let { scope.launch { runCatching { it.updateAccent(seed) } } }
    }
    /** Starts the paired PCs' beacon scan again when Bluetooth comes back on. */
    private var beaconAdapterWatch: AutoCloseable? = null
    private val network = NetworkMonitor(this.context) {
        // Network callbacks come in bursts; tell the core once they settle.
        networkChange?.cancel()
        networkChange = scope.launch {
            delay(150)
            if (lastSignal.wifiBars != null || _state.value.awayMode) {
                node?.networkChanged()
            }
            // A new Wi-Fi network turns wireless debugging off; Elevated turns it back on
            // (on a trusted network), or pauses on one that isn't.
            if (lastSignal.wifiBars != null) Elevated.onNetworkChanged()
        }
    }

    // Android drops multicast (local discovery) unless a lock is held. The
    // connection service holds one while paired; this one covers pairing.
    private val pairingMulticast: WifiManager.MulticastLock? by lazy {
        this.context.getSystemService(WifiManager::class.java)
            ?.createMulticastLock("g7link-pairing")
            ?.apply { setReferenceCounted(false) }
    }

    // Holds a multicast lock while LocalSend LAN interop (UDP 224.0.0.167:53317) is enabled.
    private val localsendMulticast: WifiManager.MulticastLock? by lazy {
        this.context.getSystemService(WifiManager::class.java)
            ?.createMulticastLock("g7link-localsend")
            ?.apply { setReferenceCounted(false) }
    }

    val state: StateFlow<CoreState> = _state.asStateFlow()

    /** True while a user-facing stream or transfer is active. */
    val busy: StateFlow<Boolean> = combine(
        _state.map { s ->
            s.transfers.any { !it.isFinished() } ||
                s.sendQueue.any { it.status is FfiQueueStatus.Sending || it.status is FfiQueueStatus.Waiting }
        }.distinctUntilChanged(),
        MirrorService.session.map { it.active }.distinctUntilChanged(),
        WebcamService.session.map { it.active || it.connecting }.distinctUntilChanged(),
        MicService.streaming.map { it != null }.distinctUntilChanged(),
        // Phone as speaker: frames arriving, or the PC still has it on
        // (frames stop while the PC is silent, for example a paused song).
        combine(pcSpeakerPlayer.playing, _state.map { it.pcSpeakerRelayOn }) { playing, relayOn ->
            playing || relayOn
        }.distinctUntilChanged(),
    ) { hasTransfers: Boolean, isMirroring: Boolean, isWebcam: Boolean, isMic: Boolean, isSpeaker: Boolean ->
        hasTransfers || isMirroring || isWebcam || isMic || isSpeaker
    }.stateIn(scope, SharingStarted.Eagerly, false)

    /** Short messages for the user (a command failed). */
    val messages: SharedFlow<String> = _messages.asSharedFlow()

    /** Undoable notices emitted when an outbound action is queued for an offline PC. */
    val queuedNotices: SharedFlow<OfflineQueuedNotice> = _queuedNotices.asSharedFlow()

    /** Context action suggestions for newly received clipboard text. */
    val clipSuggestions: SharedFlow<ClipSuggestionNotice> = _clipSuggestions.asSharedFlow()

    data class OfflineQueuedNotice(
        val message: String,
        val queueId: String?,
        val peerId: String?,
    )

    data class ClipSuggestionNotice(
        val message: String,
        val actionLabel: String,
        val suggestion: app.g7link.core.ClipSuggestion,
    )

    data class PcMirrorConfigState(
        val pcId: String,
        val codec: String,
        val width: Int,
        val height: Int,
    )

    data class PcMirrorPacket(
        val pcId: String,
        val keyframe: Boolean,
        val timeUs: Long,
        val data: ByteArray,
    )

    private val _pcMirrorConfig = MutableStateFlow<PcMirrorConfigState?>(null)
    val pcMirrorConfig: StateFlow<PcMirrorConfigState?> = _pcMirrorConfig.asStateFlow()

    @Volatile private var lastPcMirrorKeyframe: PcMirrorPacket? = null

    fun latestPcMirrorKeyframe(pcId: String): PcMirrorPacket? =
        lastPcMirrorKeyframe?.takeIf { it.pcId == pcId }

    private val _pcMirrorPackets = MutableSharedFlow<PcMirrorPacket>(
        extraBufferCapacity = 32,
        onBufferOverflow = kotlinx.coroutines.channels.BufferOverflow.DROP_OLDEST,
    )
    val pcMirrorPackets: SharedFlow<PcMirrorPacket> = _pcMirrorPackets.asSharedFlow()

    /**
     * Starts the node (once). Safe to call repeatedly, from any thread: the
     * app calls it twice as it opens, and two calls at once used to start
     * two nodes with one identity, which kept taking the PC from each other.
     */
    fun start() {
        val token = startGuard.beginStart()
        scope.launch(Dispatchers.IO) {
            lifecycleMutex.withLock {
                if (startGuard.isCancelled(token) || node != null || startJob?.isActive == true) return@withLock
                activeStartSeq = token.startSeq
                val currentRun = Job()
                runJob = currentRun
                val currentRunScope = CoroutineScope(scope.coroutineContext + currentRun)
                runScope = currentRunScope
                val job = currentRunScope.launch(Dispatchers.IO) {
            initLogging(if (BuildConfig.DEBUG) "debug,iroh=info,swarm_discovery=error" else "info,iroh=warn,swarm_discovery=error")
            val initialAwayMode = app.g7link.android.ui.Preferences.isAwayModeEnabled(context)
            val options = NodeOptions(
                dataDir = context.noBackupFilesDir.resolve("core").absolutePath,
                device = deviceInfo(context),
                appVersion = BuildConfig.VERSION_NAME,
                power = PowerLevel.BASIC,
                awayMode = initialAwayMode,
                downloadsDir = context.cacheDir.resolve("received").absolutePath,
                usbPort = 53319u.toUShort(),
            )
            val started = try {
                G7LinkNode.start(options, platform, KeystoreKeyProtector(), this@Core)
            } catch (e: G7LinkException) {
                Log.e(TAG, "core failed to start", e)
                _state.update { it.copy(status = CoreStatus.Failed(e.message ?: e.javaClass.simpleName)) }
                return@launch
            }
            if (!currentRun.isActive || startGuard.isCancelled(token)) {
                shutdownAndCloseNode(started)
                return@launch
            }
            synchronized(opsLock) {
                node = started
                isRunning = true
            }
            applyInteractive(started)
            val paired = runCatching { started.pairedDevices() }
            val devices = paired.getOrDefault(emptyList())
            val pcToggles = perPcToggles(started, devices.map { it.id })
            // The touchpad's routes live here; the core gets them at each start.
            if (paired.isSuccess) {
                app.g7link.android.ui.Preferences.keepRemoteRoutes(context, devices.map { it.id })
                app.g7link.android.ui.Preferences.keepAgentChoices(context, devices.map { it.id })
            }
            val routes = devices.associate { it.id to app.g7link.android.ui.Preferences.remoteRoute(context, it.id) }
            routes.forEach { (id, route) -> runCatching { started.setRemoteRoute(id, route) } }
            val routeStates = devices
                .mapNotNull { d -> runCatching { started.remoteRouteState(d.id) }.getOrNull()?.let { d.id to it } }
                .toMap()
            val clipHistEnabled = runCatching { started.clipboardHistoryEnabled() }.getOrDefault(true)
            val clipHist = if (clipHistEnabled) {
                runCatching { started.clipboardHistory(null) }.getOrDefault(emptyList())
            } else {
                emptyList()
            }
            val retention = runCatching { started.timelineRetention() }.getOrNull()
            val tlPage = runCatching { started.timelinePage(null, null, null, 0u, 100u) }.getOrNull()
            val lsEnabled = runCatching { started.localsendEnabled() }.getOrDefault(false)
            val lsPeers = if (lsEnabled) {
                runCatching { started.localsendPeers() }.getOrDefault(emptyList())
            } else {
                emptyList()
            }
            val initialQueue = runCatching { started.sendQueue(null).map { it.toUi() } }.getOrDefault(emptyList())
            updateLocalSendMulticast(lsEnabled)
            pruneOldTempCaches()
            _state.update {
                it.withDevices(
                    devices,
                    pcToggles.storageAllowed,
                    pcToggles.micOff,
                    pcToggles.btMediaOn,
                    pcToggles.bleLinkOn,
                ).withRoutes(routes, routeStates).copy(
                    sendQueue = initialQueue,
                    clipboardHistoryEnabled = clipHistEnabled,
                    clipboardHistory = clipHist,
                    timeline = tlPage?.items ?: emptyList(),
                    timelineTotal = tlPage?.total ?: 0u,
                    timelineHasMore = tlPage?.hasMore ?: false,
                    timelineRetentionDays = retention?.maxDays ?: 90u,
                    localsendEnabled = lsEnabled,
                    awayMode = initialAwayMode,
                    localsendPeers = lsPeers,
                )
            }
            if (devices.isNotEmpty()) ConnectionService.start(context)
            val companionAssociated = PcCompanion.isAssociated(context)
            if (companionAssociated && devices.isNotEmpty()) {
                PcCompanion.startObservingPresence(context)
            }
            _state.update {
                it.copy(status = CoreStatus.Ready(started.deviceId()))
            }
            NotificationListener.instance?.refreshActive()
            refreshDataRetention()
            app.g7link.android.clipboard.DirectShareTargets.sync(context, _state.value.devices)
            app.g7link.android.widget.PcWidgetProvider.refreshAll(context, _state.value)
            currentRunScope.launch(Dispatchers.IO) {
                // A rename in Android's settings reaches the PCs right away,
                // not at the next start (#87).
                callbackFlow {
                    val observer = object : android.database.ContentObserver(null) {
                        override fun onChange(selfChange: Boolean) {
                            trySend(Unit)
                        }
                    }
                    context.contentResolver.registerContentObserver(
                        android.provider.Settings.Global.getUriFor(android.provider.Settings.Global.DEVICE_NAME),
                        false,
                        observer,
                    )
                    awaitClose { context.contentResolver.unregisterContentObserver(observer) }
                }.collect {
                    runCatching { started.updateDeviceInfo(deviceInfo(context, _state.value.activeMediaColor)) }
                        .onFailure { Log.w(TAG, "can't send the new device name", it) }
                }
            }
            currentRunScope.launch(Dispatchers.IO) {
                _state
                    .map { s ->
                        s.devices.map { d -> listOf(d.id, d.name, d.online, d.canWake, d.wakeState) }
                    }
                    .distinctUntilChanged()
                    .collect {
                        val s = _state.value
                        app.g7link.android.clipboard.DirectShareTargets.sync(context, s.devices)
                        app.g7link.android.widget.PcWidgetProvider.refreshAll(context, s)
                    }
            }
            currentRunScope.launch(Dispatchers.Default) {
                // The volume buttons reach a PC whose volume this phone may set.
                _state
                    .map { s ->
                        s.devices
                            .filter { it.online && it.has("media.pc_audio") }
                            .mapNotNull { d -> d.deckState?.let { d.id to PcMedia.PcLevel(it.volume.toInt(), it.muted) } }
                            .toMap()
                    }
                    .distinctUntilChanged()
                    .collect { pcMedia.levels(it) }
            }
            currentRunScope.launch(Dispatchers.IO) {
                var first = true
                _state
                    .map { s ->
                        s.devices.map { d -> listOf(d.id, d.online, d.deckState?.volume, d.deckState?.muted) }
                    }
                    .distinctUntilChanged()
                    .collect {
                        if (!first) delay(120L)
                        first = false
                        app.g7link.android.widget.PcWidgetProvider.refreshAll(context, _state.value)
                    }
            }
            val initialInputEnabled = InputService.running || InputService.enabled(context)
            val initialElevated = Elevated.running
            val initialInputAccess = InputService.running || initialElevated || initialInputEnabled
            _state.update {
                it.copy(
                    companionAssociated = companionAssociated,
                    photoAccess = RecentPhotos.hasAccess(context),
                    photoPartialAccess = RecentPhotos.hasPartialAccess(context),
                    callAccess = PhoneCalls.hasAll(context),
                    contactsAccess = PhoneContacts.canRead(context),
                    smsAccess = PhoneSms.hasAll(context),
                    inputAccess = initialInputAccess,
                    elevated = initialElevated,
                    dndAccess = PhoneToggles.hasDndAccess(context),
                    writeSettingsAccess = PhoneToggles.hasWriteSettings(context),
                    storageAllFilesAccess = PhoneStorage.hasAllFilesAccess(context),
                    storageSafFolders = PhoneStorage.safFolders(context).map { f -> f.name },
                )
            }
            started.updatePower(powerLevel(), capabilities(NotificationListener.hasAccess(context)))
            syncBeaconKeys(started)
            // Elevated, when set up and wireless debugging is on.
            currentRunScope.launch { Elevated.start() }
            currentRunScope.launch(Dispatchers.Main) {
                // Allow first Compose frame to draw before registering BroadcastReceivers and ContentObservers on Main.
                delay(48L)
                battery.start()
                accent.start()
                network.start()
                beaconAdapterWatch?.close()
                beaconAdapterWatch = app.g7link.android.companion.PcBeaconScan.followAdapter(context)
                if (NotificationListener.hasAccess(context)) phoneMedia.start()
                if (PhoneCalls.canFollow(context) || PhoneCalls.canReadLog(context)) calls.start()
                if (PhoneContacts.canRead(context)) contacts.start()
                if (PhoneSms.canRead(context)) sms.start()
                _state
                    .map { s -> s.devices.any { it.online } to s.photoAccess }
                    .distinctUntilChanged()
                    .collect { (anyOnline, hasPhotoAccess) ->
                        if (anyOnline) {
                            signal.start()
                            toggles.start()
                            if (hasPhotoAccess) photos.start() else photos.stop()
                        } else {
                            signal.stop()
                            toggles.stop()
                            photos.stop()
                        }
                    }
            }
            currentRunScope.launch {
                for (op in notificationOps) {
                    runCatching { op(started) }.onFailure { Log.w(TAG, "notification update failed", it) }
                }
            }
            currentRunScope.launch {
                for (op in remoteOps) {
                    runCatching { op(started) }.onFailure { Log.w(TAG, "remote input failed", it) }
                }
            }
            currentRunScope.launch {
                for (op in pointerOps) {
                    runCatching { op(started) }.onFailure { Log.w(TAG, "pointer input failed", it) }
                }
            }
        }
        startJob = job
    }
}
}

    /**
     * Shuts down the active node and all per-run collectors, monitors, and loops.
     * Sets status back to [CoreStatus.Starting] and clears node references.
     * Non-blocking and safe to call from any thread.
     */
    fun stop() {
        val stopSeq = startGuard.recordStop()
        scope.launch(Dispatchers.IO) {
            lifecycleMutex.withLock {
                if (activeStartSeq > stopSeq) return@withLock
                val activeJob = startJob
                val activeRun = runJob
                if (activeJob == null && node == null && activeRun == null) return@withLock

                activeRun?.cancelAndJoin()
                activeJob?.cancelAndJoin()

                val activeNode = node
                synchronized(opsLock) {
                    isRunning = false
                    node = null
                    // Drain any pending operations so they don't replay on the next run.
                    while (notificationOps.tryReceive().isSuccess) {}
                    while (remoteOps.tryReceive().isSuccess) {}
                    while (pointerOps.tryReceive().isSuccess) {}
                    pendingMoves.clear()
                    pendingScrolls.clear()
                }
                startJob = null
                runJob = null
                runScope = null

                activeNode?.let { shutdownAndCloseNode(it) }
                _state.value.devices.forEach { pcMedia.update(it.id, "", emptyList()) }
                runCatching { Elevated.stop() }

                app.g7link.android.ble.PcBleLink.closeAll()
                withContext(Dispatchers.Main) {
                    beaconAdapterWatch?.close()
                    beaconAdapterWatch = null
                    battery.stop()
                    accent.stop()
                    network.stop()
                    calls.stop()
                    contacts.stop()
                    sms.stop()
                    signal.stop()
                    toggles.stop()
                    photos.stop()
                    phoneMedia.stop()
                    pcSpeakerPlayer.stop()
                    ringer.stopRinging()
                }

                lastReportedBattery = null
                pairingMulticast?.runCatching { if (isHeld) release() }
                localsendMulticast?.runCatching { if (isHeld) release() }
                wakeTimers.values.forEach { it.cancel() }
                wakeTimers.clear()
                // Their jobs ended with the run.
                bleSetups.clear()
                bleHolds.clear()
                bleHoldAsked.clear()
                bleHeld.clear()
                bleAllowedBy.clear()
                // Agent threads are kept in memory only; their notifications go with the run.
                _agents.value.keys.forEach { AgentNotifications.cancelPc(context, it) }
                _agents.value = emptyMap()
                agentWatchWanted.clear()
                agentForeground = null
                agentSessionWatchWanted.clear()
                agentSessionForeground = null
                _agentKeptSessions.value = emptySet()

                _state.update { it.withDevicesOffline().copy(status = CoreStatus.Starting) }
                app.g7link.android.widget.PcWidgetProvider.refreshAll(context, _state.value)
            }
        }
    }

    private suspend fun shutdownAndCloseNode(node: G7LinkNode) {
        try {
            node.shutdown()
        } finally {
            runCatching { node.close() }
        }
    }

    // ---- Notifications (from NotificationListener) ----

    /**
     * The notification listener connected (with what's showing now) or lost
     * access. What this phone offers PCs follows.
     */
    fun notificationAccessChanged(granted: Boolean, showing: List<Notification>) {
        _state.update { it.copy(notificationAccess = granted) }
        if (granted && _state.value.devices.isNotEmpty()) ConnectionService.start(context)
        runScope?.launch(Dispatchers.Main) { if (granted) phoneMedia.start() else phoneMedia.stop() }
        enqueueNotificationOp { node ->
            node.updatePower(powerLevel(), capabilities(granted))
            node.notificationsReset(showing)
        }
    }

    fun notificationPosted(notification: Notification) {
        enqueueNotificationOp { it.notificationPosted(notification) }
    }

    fun notificationRemoved(key: String) {
        enqueueNotificationOp { it.notificationRemoved(key) }
    }

    fun notificationsRefreshed(showing: List<Notification>) {
        enqueueNotificationOp { it.notificationsReset(showing) }
    }

    @Volatile private var lastSensitiveAccess = false
    private val refreshAccessRunning = java.util.concurrent.atomic.AtomicBoolean(false)
    private val refreshAccessPending = java.util.concurrent.atomic.AtomicBoolean(false)
    private val lastTransferUiEmitMs = java.util.concurrent.ConcurrentHashMap<String, Long>()

    /**
     * Re-checks notification access and background restrictions (the user
     * may have changed them in Settings) and asks Android to reconnect the listener if it's allowed
     * but not running. Coalesces rapid concurrent calls into at most one follow-up pass.
     */
    fun refreshNotificationAccess() {
        refreshAccessPending.set(true)
        if (!refreshAccessRunning.compareAndSet(false, true)) return
        scope.launch(Dispatchers.IO) {
            try {
                while (refreshAccessPending.compareAndSet(true, false)) {
                    val granted = NotificationListener.hasAccess(context)
                    val sensitive = granted && NotificationListener.canReadSensitive(context)
                    val becameSensitive = sensitive && !lastSensitiveAccess
                    lastSensitiveAccess = sensitive
                    val unrestricted = BackgroundAccess.isUnrestricted(context)
                    val companionAssociated = PcCompanion.isAssociated(context)
                    if (companionAssociated) {
                        PcCompanion.startObservingPresence(context)
                    }
                    // The Nearby devices permission may have just been given or taken.
                    app.g7link.android.companion.PcBeaconScan.sync(context)
                    val photoAccess = RecentPhotos.hasAccess(context)
                    val photoPartialAccess = RecentPhotos.hasPartialAccess(context)
                    val callAccess = PhoneCalls.hasAll(context)
                    val contactsAccess = PhoneContacts.canRead(context)
                    val smsAccess = PhoneSms.hasAll(context)
                    val inputEnabled = InputService.enabled(context)
                    val elevated = Elevated.running
                    val inputAccess = InputService.running || elevated || inputEnabled
                    val dndAccess = PhoneToggles.hasDndAccess(context)
                    val writeSettingsAccess = PhoneToggles.hasWriteSettings(context)
                    val storageAllFilesAccess = PhoneStorage.hasAllFilesAccess(context)
                    val storageSafFolders = PhoneStorage.safFolders(context).map { it.name }
                    if (!storageAllFilesAccess) storage.stopWatching()
                    val localNetwork = LocalNetwork.granted(context)
                    if (localNetwork && !_state.value.localNetwork) {
                        // Just allowed: reach the PCs now rather than at the next retry.
                        scope.launch { node?.networkChanged() }
                    }
                    val prevPhotos = _state.value.photoAccess
                    val currentNode = node
                    val pcToggles = currentNode?.let { perPcToggles(it, _state.value.devices.map { d -> d.id }) }
                    _state.update {
                        it.copy(
                            notificationAccess = granted,
                            backgroundUnrestricted = unrestricted,
                            companionAssociated = companionAssociated,
                            photoAccess = photoAccess,
                            photoPartialAccess = photoPartialAccess,
                            callAccess = callAccess,
                            contactsAccess = contactsAccess,
                            smsAccess = smsAccess,
                            inputAccess = inputAccess,
                            elevated = elevated,
                            remoteInputRequested = if (inputAccess) false else it.remoteInputRequested,
                            dndAccess = dndAccess,
                            writeSettingsAccess = writeSettingsAccess,
                            storageAllFilesAccess = storageAllFilesAccess,
                            storageSafFolders = storageSafFolders,
                            localNetwork = localNetwork,
                            devices = if (pcToggles != null) {
                                it.devices.map { d ->
                                    d.copy(
                                        storageEnabled = d.id in pcToggles.storageAllowed,
                                        micEnabled = d.id !in pcToggles.micOff,
                                        btMedia = d.id in pcToggles.btMediaOn,
                                        bleLink = d.id in pcToggles.bleLinkOn,
                                    )
                                }
                            } else {
                                it.devices
                            },
                        )
                    }
                    if (granted && NotificationListener.instance == null) {
                        NotificationListenerService.requestRebind(NotificationListener.component(context))
                    } else if (becameSensitive) {
                        NotificationListener.instance?.refreshActive()
                    }
                    val currentScope = runScope
                    if (node != null && currentScope != null) {
                        currentScope.launch(Dispatchers.Main) {
                            if (granted) phoneMedia.start() else phoneMedia.stop()
                            if (_state.value.devices.any { it.online }) toggles.start() else toggles.stop()
                            if (photoAccess) photos.start() else photos.stop()
                            if (PhoneCalls.canFollow(context) || PhoneCalls.canReadLog(context)) calls.start() else calls.stop()
                            if (contactsAccess) contacts.start() else contacts.stop()
                            if (PhoneSms.canRead(context)) sms.start() else sms.stop()
                        }
                        enqueueNotificationOp {
                            it.updatePower(powerLevel(), capabilities(_state.value.notificationAccess))
                            if (photoAccess && (!prevPhotos || photoPartialAccess)) it.photosChanged()
                        }
                    }
                }
            } finally {
                refreshAccessRunning.set(false)
                if (refreshAccessPending.get()) {
                    refreshNotificationAccess()
                }
            }
        }
    }

    /** Voice recordings stored on this phone and their delivery status to PCs. */
    val recordings = RecordingsStore(this.context)

    override fun onEvent(event: Event) {
        if (event is Event.Transfer) {
            val transfer = event.transfer
            val isRunningProgress = transfer.status is TransferStatus.Running &&
                transfer.done > 0uL &&
                (transfer.total == 0uL || transfer.done < transfer.total)
            if (isRunningProgress) {
                val now = android.os.SystemClock.elapsedRealtime()
                val prev = lastTransferUiEmitMs[transfer.id] ?: 0L
                val wasRunning = _state.value.transfers.firstOrNull { it.id == transfer.id }?.status is TransferStatus.Running
                if (wasRunning && now - prev < 100L) {
                    return
                }
                lastTransferUiEmitMs[transfer.id] = now
            } else {
                lastTransferUiEmitMs.remove(transfer.id)
            }
        }
        AgentsModel.pcOf(event)?.let { pcId ->
            onAgentEvent(pcId, event)
            return
        }
        val removedName = if (event is Event.DeviceRemoved) _state.value.nameOf(event.id) else null
        if (event.affectsCoreState()) {
            _state.update { it.reduce(event) }
        }
        when (event) {
            is Event.Transfer -> onTransfer(event.transfer)
            is Event.MediaChanged -> {
                val pcName = _state.value.nameOf(event.id).orEmpty()
                pcMedia.update(event.id, pcName, event.players)
            }
            is Event.NotificationPosted -> scope.launch(Dispatchers.Default) {
                val (peer, name) = peerOf(event.id)
                pcNotificationPoster.post(event.id, event.notification, peer, name)
            }
            is Event.NotificationRemoved -> scope.launch(Dispatchers.Default) {
                pcNotificationPoster.cancel(event.id, event.key)
            }
            is Event.NotificationsReset -> scope.launch(Dispatchers.Default) {
                val (peer, name) = peerOf(event.id)
                pcNotificationPoster.reset(event.id, event.items, peer, name)
            }
            is Event.PcAudioFrame -> pcSpeakerPlayer.playFrame(
                event.sampleRate.toInt(),
                event.channels.toInt(),
                event.pcm,
                event.device,
            )
            is Event.PcMirrorConfig -> {
                _pcMirrorConfig.value = PcMirrorConfigState(
                    pcId = event.id,
                    codec = event.codec,
                    width = event.width.toInt(),
                    height = event.height.toInt(),
                )
            }
            is Event.PcMirrorFrame -> {
                val pkt = PcMirrorPacket(
                    pcId = event.id,
                    keyframe = event.keyframe,
                    timeUs = event.timeUs.toLong(),
                    data = event.data,
                )
                if (pkt.keyframe) {
                    lastPcMirrorKeyframe = pkt
                }
                _pcMirrorPackets.tryEmit(pkt)
            }
            is Event.PcMirrorEnded -> {
                if (_pcMirrorConfig.value?.pcId == event.id) {
                    _pcMirrorConfig.value = null
                }
                if (lastPcMirrorKeyframe?.pcId == event.id) {
                    lastPcMirrorKeyframe = null
                }
            }
            is Event.PcCapturePrompt -> pcCapture.onPrompt(event.id)
            is Event.PcCaptureRecordEnded -> pcCapture.onRecordingEnded(event.id, event.name)
            // A PC that's gone isn't playing for this phone anymore; it
            // sends what plays when it's back.
            is Event.LinkChanged -> {
                if (event.link is Link.Offline) {
                    scope.launch(Dispatchers.Default) {
                        pcMedia.update(event.id, "", emptyList())
                    }
                    pcSpeakerPlayer.stopForPc(event.id)
                    pcCapture.onOffline(event.id)
                    agentsGone(event.id)
                    if (_pcMirrorConfig.value?.pcId == event.id) {
                        _pcMirrorConfig.value = null
                    }
                    if (lastPcMirrorKeyframe?.pcId == event.id) {
                        lastPcMirrorKeyframe = null
                    }
                } else if (event.link is Link.Online) {
                    wakeTimers.remove(event.id)?.cancel()
                    retryWaitingRecordings(event.id)
                    // Plugged into a PC with USB debugging allowed: it can start Elevated.
                    if (!Elevated.running) scope.launch(Dispatchers.IO) { Elevated.startOverUsb() }
                    scope.launch(Dispatchers.Default) {
                        accent.syncNow()
                    }
                }
            }
            is Event.DeviceAdded -> ConnectionService.start(context)
            // A PC heard of a new build: the updater checks the feed if it's for this phone.
            is Event.UpdateHint -> (context.applicationContext as? app.g7link.android.G7LinkApplication)
                ?.updater?.onHint(event.channel, event.version)
            is Event.DeviceRemoved -> {
                wakeTimers.remove(event.id)?.cancel()
                // Paired again later, it asks again until chosen again.
                PcAutoStart.set(context, event.id, false)
                app.g7link.android.ui.Preferences.removeRemoteRoute(context, event.id)
                app.g7link.android.ui.Preferences.removeAgentChoice(context, event.id)
                agentSessionsConsent.forget(event.id)
                forgetBle(event.id)
                scope.launch(Dispatchers.Default) {
                    pcMedia.update(event.id, "", emptyList())
                    pcNotificationPoster.cancelAll(event.id)
                }
                pcSpeakerPlayer.stopForPc(event.id)
                pcCapture.onOffline(event.id)
                agentsGone(event.id)
                if (_pcMirrorConfig.value?.pcId == event.id) {
                    _pcMirrorConfig.value = null
                }
                if (lastPcMirrorKeyframe?.pcId == event.id) {
                    lastPcMirrorKeyframe = null
                }
                val noDevicesRemaining = _state.value.devices.isEmpty()
                PcCompanion.disassociateForRemovedPc(context, event.id, removedName, noDevicesRemaining)
                if (noDevicesRemaining) ConnectionService.stop(context)
            }
            is Event.Paired -> ConnectionService.start(context)
            is Event.ClipboardReceived -> scope.launch(Dispatchers.IO) { onClipboardReceived(event.id) }
            is Event.ClipboardHistoryChanged -> scope.launch(Dispatchers.IO) {
                refreshClipboardHistory()
                refreshTimeline()
            }
            is Event.TimelineChanged -> scope.launch(Dispatchers.IO) { refreshTimeline() }
            is Event.LocalSendChanged -> scope.launch(Dispatchers.IO) { syncLocalSendState() }
            is Event.QueueChanged -> refreshSendQueue()
            is Event.BleKeysChanged -> node?.let { current -> scope.launch(Dispatchers.IO) { syncBeaconKeys(current) } }
            is Event.BleOutbound -> app.g7link.android.ble.PcBleLink.outbound(event.endpoint)
            is Event.QueueDroppedOnUnpair -> {
                if (event.droppedCount > 0u) {
                    val name = event.deviceName.ifBlank { context.getString(R.string.your_pc) }
                    _messages.tryEmit(
                        context.getString(
                            R.string.send_queue_dropped_unpair,
                            event.droppedCount.toInt(),
                            name,
                        ),
                    )
                }
                refreshSendQueue()
            }
            else -> {}
        }
    }

    private fun onClipboardReceived(pcId: String) {
        if (!app.g7link.android.ui.Preferences.isSuggestClipboardActionsEnabled(context)) return
        val suggestion = runCatching { node?.lastClipSuggestion() }.getOrNull() ?: return
        val pcName = _state.value.nameOf(pcId).orEmpty()
        val from = pcName.ifEmpty { context.getString(R.string.your_pc) }
        val message = context.getString(R.string.clip_received_from, from)
        val actionLabel = LinkNotifications.actionLabel(context, suggestion.kind)
        _clipSuggestions.tryEmit(ClipSuggestionNotice(message, actionLabel, suggestion))
        LinkNotifications.showClipSuggestion(context, pcName, suggestion)
    }

    /** Executes the action for a smart clipboard suggestion. */
    fun runClipSuggestion(suggestion: app.g7link.core.ClipSuggestion) {
        LinkNotifications.runSuggestion(context, suggestion)
    }

    // ---- Files & Recordings ----

    private fun onTransfer(transfer: Transfer) {
        val pc = _state.value.nameOf(transfer.deviceId).orEmpty()
        val pcOnline = _state.value.device(transfer.deviceId)?.online == true
        recordings.onTransferEvent(transfer, pcOnline)
        TransferNotifications.update(context, transfer, pc)
        if (transfer.status is TransferStatus.Failed || transfer.status is TransferStatus.Cancelled) {
            pcCapture.onTransferGone(transfer.id)
        }
        val done = transfer.status as? TransferStatus.Done ?: return
        if (transfer.direction != TransferDirection.INCOMING) {
            scope.launch(Dispatchers.IO) { refreshDataRetention() }
            return
        }
        // A screenshot this phone asked a PC for opens to crop; it doesn't land in Downloads.
        if (pcCapture.claim(transfer, done.saved)) return
        // Out of the app's cache, into Downloads, then tell the user.
        scope.launch(Dispatchers.IO) {
            val published = done.saved.flatMap { ReceivedFiles.publish(context, java.io.File(it)) }
            if (published.isNotEmpty()) {
                if (published.size == 1 && transfer.files <= 1u && !transfer.recording) {
                    val first = published.first()
                    val label = transfer.names.firstOrNull()?.takeIf(String::isNotBlank)
                        ?: first.name.ifBlank { "File" }
                    PhoneClipboard.setFileUri(context, first.uri, label)
                }
                val title = transferTitle(context.resources, transfer)
                val detail = if (transfer.files > 1u) {
                    context.resources.getQuantityString(R.plurals.transfer_files, transfer.files.toInt(), transfer.files.toInt())
                } else {
                    ""
                }
                node?.updateTimelineByRef(
                    transfer.id,
                    TimelineKind.FILE,
                    title,
                    detail,
                    published.first().uri.toString(),
                    transfer.total,
                )
                refreshTimeline()
            } else {
                refreshDataRetention()
            }
            TransferNotifications.received(context, transfer, published, pc)
        }
    }

    /** Saves a finished voice recording and sends it to the chosen PC. */
    fun onRecordingFinished(
        pcId: String,
        file: java.io.File,
        durationMs: Long,
        markers: List<RecordingMarker>,
    ): SavedRecording {
        val online = _state.value.device(pcId)?.online == true
        val saved = recordings.add(
            file = file,
            durationMs = durationMs,
            markers = markers,
            targetPcId = pcId,
            waiting = !online,
        )
        sendSavedRecording(pcId, saved.id)
        return saved
    }

    /** Sends (or re-sends) a saved voice recording and its markers to `pcId`. */
    fun sendSavedRecording(pcId: String, recordingId: String) {
        val rec = recordings.get(recordingId) ?: return
        val file = recordings.fileFor(rec)
        if (!file.exists()) return
        val online = _state.value.device(pcId)?.online == true
        recordings.updateDelivery(
            id = recordingId,
            pcId = pcId,
            transferId = rec.transferId,
            delivery = if (online) DeliveryState.Sending else DeliveryState.Waiting,
        )
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val node = node ?: return@launch
            try {
                val tid = node.sendRecording(
                    pcId,
                    FileToSend.Path(
                        path = file.absolutePath,
                        name = file.name,
                        folder = null,
                    ),
                    rec.markers,
                )
                val nowOnline = _state.value.device(pcId)?.online == true
                val existing = _state.value.transfers.firstOrNull { it.id == tid }
                val delivery = when (existing?.status) {
                    is TransferStatus.Done -> DeliveryState.Sent
                    is TransferStatus.Running -> DeliveryState.Sending
                    is TransferStatus.Waiting -> DeliveryState.Waiting
                    else -> if (nowOnline) DeliveryState.Sending else DeliveryState.Waiting
                }
                recordings.updateDelivery(
                    id = recordingId,
                    pcId = pcId,
                    transferId = tid,
                    delivery = delivery,
                )
            } catch (e: G7LinkException) {
                val name = _state.value.nameOf(pcId).orEmpty()
                when (e) {
                    is G7LinkException.Denied -> {
                        recordings.updateDelivery(recordingId, pcId, null, DeliveryState.Denied)
                        _messages.tryEmit(context.getString(R.string.recorder_denied_text, name))
                    }
                    is G7LinkException.Offline -> {
                        recordings.updateDelivery(recordingId, pcId, null, DeliveryState.Waiting)
                    }
                    else -> {
                        recordings.updateDelivery(recordingId, pcId, null, DeliveryState.Failed)
                        _messages.tryEmit(describe(e))
                    }
                }
            }
        }
    }

    private fun retryWaitingRecordings(pcId: String) {
        val activeIds = _state.value.transfers.filterNot { it.isFinished() }.map { it.id }.toSet()
        for (rec in recordings.waitingForPc(pcId)) {
            if (rec.transferId == null || rec.transferId !in activeIds) {
                sendSavedRecording(pcId, rec.id)
            }
        }
    }

    /** Opens this phone's screen stream to a PC (after the user agreed). */
    suspend fun mirrorOpen(pcId: String): MirrorStream {
        startJob?.join()
        val node = checkNotNull(node) { "the core isn't running" }
        return node.mirrorOpen(pcId)
    }

    /** Opens this phone's sound stream to a PC that asked for it with the screen. */
    suspend fun mirrorOpenAudio(pcId: String): MirrorStream {
        startJob?.join()
        val node = checkNotNull(node) { "the core isn't running" }
        return node.mirrorOpenAudio(pcId)
    }

    /**
     * Elevated through a PC's adb (`docs/protocol/elevated-usb.md`): asks each
     * connected PC to start this phone's helper. One it's plugged into with
     * USB debugging allowed can; so can one this phone's Wireless debugging
     * has paired, when told the port it listens on ([adbPort]). Returns the
     * helper's port, or `null`.
     */
    suspend fun elevatedUsbStart(apk: String, token: String, grants: List<String>, adbPort: Int? = null): Int? {
        startJob?.join()
        val node = node ?: return null
        for (pc in _state.value.devices.filter { it.online }) {
            val port = runCatching {
                node.elevatedUsbStart(pc.id, apk, token, grants, adbPort?.toUShort())
            }.onFailure { Log.i(TAG, "a PC couldn't start Elevated: ${it.message}") }.getOrNull()
            if (port != null) return port.toInt()
        }
        return null
    }

    /** Accepted codecs from the connected PC; an unknown receiver gets H.264. */
    suspend fun webcamCodecs(pcId: String): List<String> {
        startJob?.join()
        return runCatching { node?.webcamCodecs(pcId) }.getOrNull() ?: listOf("h264")
    }

    /** Accepted screen codecs from the connected PC; unknown receivers use H.264. */
    suspend fun mirrorCodecs(pcId: String): List<String> {
        startJob?.join()
        return runCatching { node?.mirrorCodecs(pcId) }.getOrNull() ?: listOf("h264")
    }

    /** Opens this phone's camera stream to a PC (`webcam`), or emits a message on failure. */
    suspend fun webcamOpenOrMessage(pcId: String): MirrorStream? {
        startJob?.join()
        val node = node ?: return null
        return try {
            node.webcamOpen(pcId)
        } catch (e: G7LinkException) {
            val name = _state.value.nameOf(pcId).orEmpty()
            val msg = when (e) {
                is G7LinkException.Denied -> context.getString(R.string.webcam_denied_text, name)
                else -> describe(e)
            }
            _messages.tryEmit(msg)
            null
        }
    }

    /** Opens this phone's microphone stream to [pcId] (docs/protocol/mic.md); null if it can't. */
    suspend fun micOpen(pcId: String): MirrorStream? {
        startJob?.join()
        val node = node ?: return null
        return try {
            node.micOpen(pcId)
        } catch (e: G7LinkException) {
            _messages.tryEmit(describe(e))
            null
        }
    }

    /** Whether [id] is a paired phone or tablet (not a PC), and its name. */
    private fun peerOf(id: String): Pair<Boolean, String> {
        val device = _state.value.devices.firstOrNull { it.id == id }
        return (device?.isAndroid == true) to device?.name.orEmpty()
    }

    /** Dismisses a paired phone's or tablet's notification there (dismissed here). */
    fun dismissPeerNotification(id: String, key: String) {
        command { it.dismissPeerNotification(id, key) }
    }

    /** Stops this phone's microphone stream to [pcId] and tells the PC. */
    fun micStop(pcId: String) {
        command { it.micStop(pcId) }
    }

    /** The PC's request for the microphone is answered or withdrawn: take it off Home and the shade. */
    fun clearMicRequest(pcId: String) {
        app.g7link.android.mic.MicRequests.dismiss(context, pcId)
        _state.update { if (it.micRequestFrom == pcId) it.copy(micRequestFrom = null) else it }
    }

    /** Not now: the PC stops waiting. */
    fun declineMicRequest(pcId: String) {
        clearMicRequest(pcId)
        micStop(pcId)
    }

    /**
     * Lets [pcId] ask for this phone's microphone, or not (on by default). Turning it off
     * also ends a stream to that PC, and the core tells the PC.
     */
    fun setMicEnabled(pcId: String, enabled: Boolean) {
        _state.update { it.withMicEnabled(pcId, enabled) }
        if (!enabled) app.g7link.android.mic.MicRequests.dismiss(context, pcId)
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            runCatching { currentNode.setDeviceToggle(pcId, app.g7link.android.mic.MicService.TOGGLE_NAME, enabled) }
                .onFailure { Log.w(TAG, "can't change the microphone toggle", it) }
        }
    }

    /**
     * Lets this phone play its media on [pcId] over Bluetooth, or keeps it on the phone (off by
     * default). The core tells the PC, which keeps the same.
     */
    fun setBtMedia(pcId: String, on: Boolean) {
        _state.update { it.withBtMedia(pcId, on) }
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            runCatching { currentNode.setDeviceToggle(pcId, BT_MEDIA_TOGGLE, on) }
                .onFailure { Log.w(TAG, "can't change the Bluetooth media toggle", it) }
        }
    }

    /**
     * Which PCs may browse this phone's storage, which may not ask for its microphone, and which
     * it may play its media on over Bluetooth.
     */
    private class PerPcToggles(
        val storageAllowed: Set<String>,
        val micOff: Set<String>,
        val btMediaOn: Set<String>,
        val bleLinkOn: Set<String>,
    )

    private fun perPcToggles(node: G7LinkNode, ids: List<String>): PerPcToggles {
        val storage = mutableSetOf<String>()
        val micOff = mutableSetOf<String>()
        val btMediaOn = mutableSetOf<String>()
        val bleLinkOn = mutableSetOf<String>()
        for (id in ids) {
            val toggles = runCatching { node.deviceToggles(id) }.getOrNull() ?: continue
            if (toggles.any { it.name == PhoneStorage.TOGGLE_NAME && it.enabled }) storage += id
            if (toggles.any { it.name == app.g7link.android.mic.MicService.TOGGLE_NAME && !it.enabled }) micOff += id
            if (toggles.any { it.name == BT_MEDIA_TOGGLE && it.enabled }) btMediaOn += id
            if (toggles.any { it.name == BLE_LINK_TOGGLE && it.enabled }) bleLinkOn += id
        }
        return PerPcToggles(storage, micOff, btMediaOn, bleLinkOn)
    }

    fun clearWebcamRequest(pcId: String? = null) {
        if (pcId != null) WebcamRequests.dismiss(context, pcId)
        _state.update { s ->
            if (pcId == null || s.webcamRequest?.pcId == pcId) s.copy(webcamRequest = null) else s
        }
    }

    /** Sends a captured photo or scanned document back to the requesting PC (`camera.result`). */
    fun sendCameraCaptureResult(
        pcId: String,
        requestId: String,
        mode: String,
        fileName: String,
        mime: String,
        width: Int,
        height: Int,
        data: ByteArray,
    ) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            try {
                currentNode.sendCameraCaptureResult(
                    pcId = pcId,
                    requestId = requestId,
                    mode = mode,
                    fileName = fileName,
                    mime = mime,
                    width = width.coerceAtLeast(1).toUInt(),
                    height = height.coerceAtLeast(1).toUInt(),
                    data = data,
                )
            } catch (e: G7LinkException) {
                _messages.tryEmit(describe(e))
            }
        }
    }

    /** Notifies the requesting PC that a Continuity Camera capture was cancelled (`camera.cancel`). */
    fun cancelCameraCapture(pcId: String, requestId: String, reason: String? = null) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.cancelCameraCapture(pcId, requestId, reason) }
        }
    }

    /**
     * Sends picked or shared files to a PC; problems arrive as messages.
     */
    fun sendFiles(pcId: String, uris: List<Uri>, handoff: Boolean = false) {
        scope.launch(Dispatchers.IO) {
            val files = try {
                OutgoingFiles.open(context, uris)
            } catch (_: OutgoingFiles.TooManyFiles) {
                _messages.tryEmit(context.getString(R.string.transfer_too_many))
                return@launch
            }
            send(pcId, files, handoff)
        }
    }

    /** Sends a folder the user picked, with everything in it. */
    fun sendFolder(pcId: String, tree: Uri) {
        scope.launch(Dispatchers.IO) {
            val files = try {
                OutgoingFiles.openFolder(context, tree)
            } catch (_: OutgoingFiles.TooManyFiles) {
                _messages.tryEmit(context.getString(R.string.transfer_too_many))
                return@launch
            } catch (e: Exception) {
                Log.w(TAG, "can't read a picked folder", e)
                emptyList()
            }
            send(pcId, files)
        }
    }

    private fun send(pcId: String, files: List<FileToSend>, handoff: Boolean = false) {
        if (files.isEmpty()) {
            _messages.tryEmit(context.getString(R.string.transfer_nothing))
            return
        }
        scope.launch(Dispatchers.IO) {
            var handedToCore = false
            try {
                startJob?.join()
                val node = node ?: return@launch
                val wasOnline = _state.value.device(pcId)?.online == true
                try {
                    val singleFileName = if (files.size == 1) {
                        when (val f = files[0]) {
                            is FileToSend.Fd -> f.name.takeIf { f.folder == null }
                            is FileToSend.Path -> f.name.takeIf { f.folder == null }
                        }
                    } else {
                        null
                    }
                    handedToCore = true
                    val returnedId = if (handoff && singleFileName != null && app.g7link.core.isSafeHandoffDocument(singleFileName)) {
                        node.sendHandoffFiles(pcId, files)
                    } else {
                        node.sendFiles(pcId, files)
                    }
                    refreshSendQueue()
                    if (!wasOnline) {
                        val name = _state.value.nameOf(pcId).orEmpty()
                        val qid = runCatching { node.latestQueuedIdForPeer(pcId) }.getOrNull()
                            ?: returnedId.takeIf { it.isNotBlank() }
                        emitOfflineQueuedNotice(pcId, name, qid)
                    }
                } catch (e: G7LinkException) {
                    val name = _state.value.nameOf(pcId).orEmpty()
                    _messages.tryEmit(
                        if (e is G7LinkException.Denied) context.getString(R.string.transfer_off_for, name) else describe(e),
                    )
                }
            } finally {
                if (!handedToCore) {
                    OutgoingFiles.closeAll(files)
                }
            }
        }
    }

    fun acceptTransfer(id: String) {
        scope.launch(Dispatchers.IO) { runCatching { node?.acceptTransfer(id) } }
    }

    fun cancelTransfer(id: String) {
        scope.launch(Dispatchers.IO) { runCatching { node?.cancelTransfer(id) } }
    }

    fun pauseTransfer(id: String) {
        scope.launch(Dispatchers.IO) { runCatching { node?.pauseTransfer(id) } }
    }

    fun resumeTransfer(id: String) {
        scope.launch(Dispatchers.IO) { runCatching { node?.resumeTransfer(id) } }
    }

    // ---- Send Queue ----

    private val refreshQueueRunning = java.util.concurrent.atomic.AtomicBoolean(false)
    private val refreshQueuePending = java.util.concurrent.atomic.AtomicBoolean(false)

    fun refreshSendQueue() {
        refreshQueuePending.set(true)
        if (!refreshQueueRunning.compareAndSet(false, true)) return
        scope.launch(Dispatchers.IO) {
            try {
                while (refreshQueuePending.compareAndSet(true, false)) {
                    val currentNode = node ?: break
                    val items = runCatching { currentNode.sendQueue(null).map { it.toUi() } }.getOrDefault(emptyList())
                    _state.update { prev ->
                        val merged = items.map { item ->
                            if (item.kind.isFileBacked() && item.status is app.g7link.core.FfiQueueStatus.Sending) {
                                val targetId = item.activeTransferId ?: item.id
                                val liveTransfer = prev.transfers.firstOrNull {
                                    it.id == targetId && it.direction == TransferDirection.OUTGOING
                                }
                                val prevItem = prev.sendQueue.firstOrNull { it.id == item.id }
                                val liveDone = maxOf(
                                    item.bytesDone,
                                    liveTransfer?.done?.toLong() ?: 0L,
                                    prevItem?.bytesDone ?: 0L,
                                )
                                val liveTotal = if (item.totalBytes > 0L) {
                                    item.totalBytes
                                } else {
                                    liveTransfer?.total?.toLong() ?: prevItem?.totalBytes ?: 0L
                                }
                                item.copy(
                                    totalBytes = liveTotal.coerceAtLeast(0L),
                                    bytesDone = liveDone.coerceIn(0L, liveTotal.coerceAtLeast(0L)),
                                )
                            } else {
                                item
                            }
                        }
                        prev.copy(sendQueue = merged)
                    }
                }
            } finally {
                refreshQueueRunning.set(false)
                if (refreshQueuePending.get()) {
                    refreshSendQueue()
                }
            }
        }
    }

    fun cancelQueueItem(id: String) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.cancelQueueItem(id) }
            refreshSendQueue()
        }
    }

    fun cancelAllQueueItems(peerId: String? = null) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.cancelAllQueueItems(peerId) }
            refreshSendQueue()
        }
    }

    fun retryQueueItem(id: String) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.retryQueueItem(id) }
            refreshSendQueue()
        }
    }

    fun clearFinishedQueueItems(peerId: String? = null) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.clearFinishedQueueItems(peerId) }
            refreshSendQueue()
        }
    }

    private fun emitOfflineQueuedNotice(pcId: String, pcName: String, queueId: String? = null) {
        val resolvedQueueId = queueId ?: runCatching { node?.latestQueuedIdForPeer(pcId) }.getOrNull()
        val displayName = pcName.ifBlank { context.getString(R.string.your_pc) }
        val message = context.getString(R.string.send_queue_offline_toast, displayName)
        _queuedNotices.tryEmit(
            OfflineQueuedNotice(
                message = message,
                queueId = resolvedQueueId,
                peerId = pcId,
            ),
        )
    }

    /**
     * Whether the user has the app open: the core checks its links every 5 s
     * then instead of every 15 s, and right away when it opens.
     */
    fun setInteractive(on: Boolean) {
        interactive = on
        val current = node ?: return
        scope.launch { applyInteractive(current) }
    }

    /** Hands the node the latest value, so calls that run out of order can't leave an older one. */
    private fun applyInteractive(target: G7LinkNode) {
        synchronized(interactiveLock) {
            runCatching { target.setInteractive(interactive) }
        }
    }

    // ---- LocalSend interop & Mobile Data (Away mode) ----

    fun setAwayMode(enabled: Boolean) {
        app.g7link.android.ui.Preferences.setAwayModeEnabled(context, enabled)
        _state.update { it.copy(awayMode = enabled) }
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.setAwayMode(enabled) }
        }
    }

    internal fun onScreenInteractiveChanged(interactive: Boolean) {
        updateLocalSendMulticast(_state.value.localsendEnabled, interactive)
    }

    private fun updateLocalSendMulticast(
        enabled: Boolean,
        interactive: Boolean = context.getSystemService(android.os.PowerManager::class.java)?.isInteractive != false,
    ) {
        val shouldHold = enabled && interactive && isRunning
        localsendMulticast?.runCatching { if (shouldHold) { if (!isHeld) acquire() } else if (isHeld) release() }
    }

    suspend fun relaySnapshot(): RelaySnapshot? = withContext(Dispatchers.IO) {
        runCatching { node?.relaySnapshot() }.getOrNull()
    }

    suspend fun editRelay(url: String, action: RelayEdit): RelayEditResult = withContext(Dispatchers.IO) {
        runCatching { node?.editRelay(url, action) }.getOrNull() ?: RelayEditResult.FAILED
    }

    suspend fun checkRelays(): List<RelayProbe> = try {
        node?.checkRelays().orEmpty()
    } catch (e: CancellationException) {
        throw e
    } catch (e: IllegalStateException) {
        // The node can be closed while the settings dialog is open.
        emptyList()
    }

    fun setLocalSendEnabled(enabled: Boolean) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            try {
                currentNode.setLocalsendEnabled(enabled)
            } catch (e: G7LinkException) {
                _messages.tryEmit(describe(e))
            }
            updateLocalSendMulticast(enabled)
            syncLocalSendState()
        }
    }

    fun refreshLocalSend() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            runCatching { currentNode.refreshLocalsend() }
            syncLocalSendState()
        }
    }

    private fun syncLocalSendState() {
        val currentNode = node ?: return
        val enabled = runCatching { currentNode.localsendEnabled() }.getOrDefault(false)
        val peers = if (enabled) {
            runCatching { currentNode.localsendPeers() }.getOrDefault(emptyList())
        } else {
            emptyList()
        }
        updateLocalSendMulticast(enabled)
        _state.update { it.copy(localsendEnabled = enabled, localsendPeers = peers) }
    }

    // ---- Clipboard ----

    @Volatile private var lastAutoSentClip: Pair<String, String?>? = null
    @Volatile private var lastAutoSentAtMs: Long = 0L

    /** What a clipboard send came to; [message] is what to tell the user. */
    sealed interface ClipSend {
        val message: String

        /** On these PCs' clipboards now (any others it went to queued it). */
        data class Sent(val pcIds: List<String>, override val message: String) : ClipSend

        /** Waiting in the send queue until the PC is back. */
        data class Queued(override val message: String) : ClipSend

        data class Failed(override val message: String) : ClipSend
    }

    /**
     * Sends text (and optional HTML) to every connected PC (or to `pcId` when chosen via Direct Share).
     * Waits for the core when the app was just launched.
     */
    suspend fun sendClipboard(text: String, pcId: String? = null, html: String? = null): ClipSend =
        sendToPcs(image = false, pcId = pcId) { node, pc -> node.sendClipboardRich(pc, text, html, null) }

    /**
     * Runs a clipboard send ([sendClipboard], [sendClipboardImage] or [sendClipboardSensitive])
     * in the background and returns at once; what it says shows as a toast.
     */
    fun sendClipboardInBackground(send: suspend Core.() -> ClipSend) {
        scope.launch {
            val message = try {
                send().message
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "clipboard not sent", e)
                context.getString(R.string.error_generic)
            }
            if (message.isNotBlank()) {
                withContext(Dispatchers.Main) { Toast.makeText(context, message, Toast.LENGTH_SHORT).show() }
            }
        }
    }

    /**
     * Starts a clipboard send ([sendClipboard], [sendClipboardImage] or [sendClipboardSensitive])
     * that keeps going when the screen that asked for it closes.
     */
    fun startClipboardSend(send: suspend Core.() -> ClipSend): Deferred<ClipSend> =
        scope.async {
            try {
                send()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "clipboard not sent", e)
                ClipSend.Failed(context.getString(R.string.error_generic))
            }
        }

    /** Says how a send from [startClipboardSend] ended in a toast, once the screen that started it is gone. */
    fun toastWhenSent(send: Deferred<ClipSend>) {
        scope.launch {
            val message = send.await().message
            if (message.isNotBlank()) {
                withContext(Dispatchers.Main) { Toast.makeText(context, message, Toast.LENGTH_SHORT).show() }
            }
        }
    }

    /** Sends what a password manager copied, when the user chose that. Never queued. */
    suspend fun sendClipboardSensitive(text: String, pcId: String? = null): ClipSend =
        sendToPcs(image = false, pcId = pcId) { node, pc -> node.sendClipboardPrivate(pc, text) }

    /** A password manager copied this in the background while Elevated is running. */
    fun onElevatedSensitiveCopied(text: String) {
        if (!PhoneClipboard.syncPasswords(context) || PhoneClipboard.isRecentEcho(text, null)) return
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            for (pc in _state.value.devices.filter { it.online }) {
                runCatching { currentNode.sendClipboardPrivate(pc.id, text) }
                    .onFailure { Log.d(TAG, "private clip not sent to ${pc.id}") }
            }
        }
    }

    /**
     * Automatically sends non-sensitive text copied in the background while Elevated mode is running.
     * Skips clips G7 Link itself wrote from a PC (`PhoneClipboard.isRecentEcho`).
     */
    /** Elevated's taps are refused: tells the PC that sent the last ones what to turn on. */
    fun onElevatedInputBlocked() {
        val (pcId, session) = PhonePlatform.lastMirrorInput ?: return
        scope.launch(Dispatchers.IO) {
            runCatching { node?.mirrorInputBlocked(pcId, session) }
        }
    }

    fun onElevatedClipboardChanged(text: String, html: String?) {
        if (text.isBlank() || PhoneClipboard.isRecentEcho(text, html)) return
        val now = android.os.SystemClock.elapsedRealtime()
        val key = text to html
        if (lastAutoSentClip == key && (now - lastAutoSentAtMs) in 0..1_500L) return
        lastAutoSentClip = key
        lastAutoSentAtMs = now
        PhoneClipboard.recordLocalCopy(text, html)
        scope.launch(Dispatchers.IO) {
            if (Build.FINGERPRINT.startsWith("generic") || Build.HARDWARE.contains("ranchu")) {
                delay(80)
            }
            startJob?.join()
            val currentNode = node ?: return@launch
            val onlinePcs = _state.value.devices.filter { it.online }
            for (pc in onlinePcs) {
                runCatching { currentNode.sendClipboardRich(pc.id, text, html, null) }
                    .onFailure { Log.d(TAG, "auto clipboard not sent to ${pc.id}: $it") }
            }
        }
    }

    private fun targetPcsForSend(pcId: String?): List<Device> {
        val all = _state.value.devices
        if (pcId != null) {
            return all.filter { it.id == pcId }
        }
        val online = all.filter { it.online }
        return if (online.isNotEmpty()) online else all
    }

    /** Sends a copied image to the connected PCs (or to `pcId`, queueing if offline). */
    suspend fun sendClipboardImage(uri: Uri, pcId: String? = null): ClipSend {
        startJob?.join()
        val targets = targetPcsForSend(pcId)
        if (targets.isEmpty()) return ClipSend.Failed(context.getString(R.string.clip_no_pc))
        val image = when (val read = withContext(Dispatchers.IO) { PhoneClipboard.readImage(context, uri) }) {
            is PhoneClipboard.ImageResult.Ready -> read.image
            PhoneClipboard.ImageResult.TooLarge -> return ClipSend.Failed(context.getString(R.string.clip_image_too_large))
            PhoneClipboard.ImageResult.Unreadable -> return ClipSend.Failed(context.getString(R.string.clip_image_unreadable))
        }
        return sendToPcs(image = true, pcId = pcId) { node, pc -> node.sendClipboardImage(pc, image.mime, image.bytes) }
    }

    /** Puts a PNG (a screenshot of the PC, cropped here) on a PC's clipboard; returns what to tell the user. */
    suspend fun sendClipboardPng(pcId: String, png: ByteArray): String {
        if (png.size > MAX_CLIPBOARD_PNG_BYTES) return context.getString(R.string.clip_image_too_large)
        return sendToPcs(image = true, pcId = pcId) { node, pc -> node.sendClipboardImage(pc, "image/png", png) }.message
    }

    /**
     * Pastes an image where PC [pcId] is typing: onto its clipboard (sized to
     * paste, the right way up), then the paste key, once the PC says its
     * clipboard has it. Only to a PC that's connected now, so nothing pastes
     * later somewhere else. Returns whether it was pasted; says why not.
     */
    suspend fun pasteImageOnPc(pcId: String, uri: Uri, onStatus: ((RemoteAccess) -> Unit)? = null): Boolean {
        val pc = _state.value.device(pcId)
        if (pc == null || !pc.online) {
            _messages.tryEmit(context.getString(R.string.clip_no_pc))
            return false
        }
        val image = when (val read = withContext(Dispatchers.IO) { PhoneClipboard.readImageForPaste(context, uri) }) {
            is PhoneClipboard.ImageResult.Ready -> read.image
            PhoneClipboard.ImageResult.TooLarge -> {
                _messages.tryEmit(context.getString(R.string.clip_image_too_large))
                return false
            }
            PhoneClipboard.ImageResult.Unreadable -> {
                _messages.tryEmit(context.getString(R.string.clip_image_unreadable))
                return false
            }
        }
        val node = node ?: return false
        try {
            node.pasteImage(pcId, image.mime, image.bytes)
        } catch (e: G7LinkException) {
            _messages.tryEmit(
                when (e) {
                    is G7LinkException.Denied -> context.getString(R.string.clip_off_for, pc.name)
                    is G7LinkException.Internal -> context.getString(R.string.remote_image_not_pasted, pc.name)
                    else -> describe(e)
                },
            )
            return false
        }
        // The PC's paste key: Ctrl+Shift+V in a terminal.
        remoteKey(pcId, "paste", emptyList(), onStatus)
        return true
    }

    private suspend fun sendToPcs(
        image: Boolean,
        pcId: String? = null,
        send: suspend (G7LinkNode, String) -> Unit,
    ): ClipSend {
        startJob?.join()
        val node = node ?: return ClipSend.Failed(context.getString(R.string.clip_no_pc))
        val pcs = targetPcsForSend(pcId)
        if (pcs.isEmpty()) return ClipSend.Failed(context.getString(R.string.clip_no_pc))
        val sentTo = mutableListOf<Device>()
        val queuedTo = mutableListOf<Device>()
        var lastQueueId: String? = null
        var failure: String? = null
        for (pc in pcs) {
            val wasOnline = pc.online
            try {
                send(node, pc.id)
                if (wasOnline) {
                    sentTo += pc
                } else {
                    queuedTo += pc
                    lastQueueId = runCatching { node.latestQueuedIdForPeer(pc.id) }.getOrNull()
                }
            } catch (e: G7LinkException) {
                failure = when (e) {
                    is G7LinkException.Denied -> context.getString(R.string.clip_off_for, pc.name)
                    is G7LinkException.TooLarge -> context.getString(
                        if (image) R.string.clip_image_too_large else R.string.clip_too_large,
                    )
                    is G7LinkException.Unsupported ->
                        if (image) context.getString(R.string.clip_image_unsupported, pc.name) else describe(e)
                    else -> describe(e)
                }
            }
        }
        if (queuedTo.isNotEmpty()) {
            refreshSendQueue()
            val names = queuedTo.joinToString { it.name }
            val msg = context.getString(R.string.send_queue_offline_toast, names)
            _queuedNotices.tryEmit(
                OfflineQueuedNotice(
                    message = msg,
                    queueId = if (queuedTo.size == 1) lastQueueId else null,
                    peerId = queuedTo.singleOrNull()?.id,
                ),
            )
            if (sentTo.isEmpty()) {
                return ClipSend.Queued(msg)
            }
        }
        return if (sentTo.isNotEmpty()) {
            ClipSend.Sent(sentTo.map { it.id }, context.getString(R.string.clip_sent_to, sentTo.joinToString { it.name }))
        } else {
            ClipSend.Failed(failure.orEmpty())
        }
    }

    private fun refreshClipboardHistory() {
        val currentNode = node ?: return
        val enabled = runCatching { currentNode.clipboardHistoryEnabled() }.getOrDefault(true)
        val items = if (enabled) {
            runCatching { currentNode.clipboardHistory(null) }.getOrDefault(emptyList())
        } else {
            emptyList()
        }
        _state.update { it.copy(clipboardHistoryEnabled = enabled, clipboardHistory = items) }
        refreshDataRetention()
    }

    fun setClipboardHistoryEnabled(enabled: Boolean) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            runCatching { currentNode.setClipboardHistoryEnabled(enabled) }
            refreshClipboardHistory()
        }
    }

    fun copyClipboardHistory(id: String) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            try {
                currentNode.copyClipboardHistory(id)
                // Android 13 and later confirm copies themselves.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                    _messages.tryEmit(context.getString(R.string.clipboard_history_copied))
                }
            } catch (e: G7LinkException) {
                _messages.tryEmit(describe(e))
            }
        }
    }

    fun pinClipboardHistory(id: String, pinned: Boolean) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            node?.pinClipboardHistory(id, pinned)
            refreshClipboardHistory()
        }
    }

    fun deleteClipboardHistory(id: String) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            node?.deleteClipboardHistory(id)
            refreshClipboardHistory()
        }
    }

    fun clearClipboardHistory() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.clearClipboardHistory() }
            refreshClipboardHistory()
        }
    }

    suspend fun clipboardHistoryImage(id: String): ByteArray? = withContext(Dispatchers.IO) {
        startJob?.join()
        node?.clipboardHistoryImage(id)
    }

    // ---- Timeline ----

    private fun refreshTimeline() {
        val currentNode = node ?: return
        val retention = runCatching { currentNode.timelineRetention() }.getOrNull()
        val page = runCatching { currentNode.timelinePage(null, null, null, 0u, 100u) }.getOrNull()
        _state.update {
            it.copy(
                timeline = page?.items ?: it.timeline,
                timelineTotal = page?.total ?: it.timelineTotal,
                timelineHasMore = page?.hasMore ?: it.timelineHasMore,
                timelineRetentionDays = retention?.maxDays ?: it.timelineRetentionDays,
            )
        }
        refreshDataRetention()
    }

    suspend fun queryTimelinePage(
        kind: TimelineKind?,
        deviceId: String?,
        search: String?,
        offset: UInt,
        limit: UInt = 100u,
    ): TimelinePage? = withContext(Dispatchers.IO) {
        startJob?.join()
        runCatching { node?.timelinePage(kind, deviceId, search, offset, limit) }.getOrNull()
    }

    fun setTimelineRetentionDays(days: UInt) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.setTimelineRetention(days, 5_000u) }
            refreshTimeline()
        }
    }

    fun deleteTimelineEntry(id: Long) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.deleteTimelineEntry(id) }
            refreshTimeline()
        }
    }

    fun clearTimeline() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            runCatching { node?.clearTimeline() }.onFailure {
                Log.w(TAG, "couldn't clear the Timeline", it)
                _messages.tryEmit(context.getString(R.string.data_clear_failed))
            }
            refreshTimeline()
        }
    }

    // ---- Data retention & Clear everything ----

    private fun pruneOldTempCaches() {
        val cutoff = System.currentTimeMillis() - 24L * 60 * 60 * 1000
        listOf("continuity_camera", "clipboard").forEach { dirName ->
            java.io.File(context.cacheDir, dirName).listFiles()?.forEach { file ->
                if (file.isFile && file.lastModified() in 1 until cutoff) {
                    runCatching { file.delete() }
                }
            }
        }
    }

    private fun refreshDataRetention() {
        val counts = runCatching { node?.dataRetentionCounts() }.getOrNull()
        val photoFiles = listOf("continuity_camera", "clipboard").sumOf { dirName ->
            java.io.File(context.cacheDir, dirName).listFiles()?.count { it.isFile } ?: 0
        }
        val mmsFiles = java.io.File(context.cacheDir, "mms")
            .listFiles()
            ?.count { it.isFile && it.name.endsWith(".mms", ignoreCase = true) } ?: 0
        _state.update {
            it.copy(
                receivedFileRecords = counts?.receivedFileRecords?.toInt() ?: it.receivedFileRecords,
                cachedPhotoFiles = photoFiles,
                cachedMmsFiles = mmsFiles,
            )
        }
    }

    fun clearMessageCache() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            java.io.File(context.cacheDir, "mms").listFiles()?.forEach { file ->
                if (file.isFile && file.name.endsWith(".mms", ignoreCase = true)) {
                    runCatching { file.delete() }
                }
            }
            runCatching { node?.clearMessageCache() }.onFailure {
                Log.w(TAG, "couldn't clear the message cache", it)
                _messages.tryEmit(context.getString(R.string.data_clear_failed))
            }
            refreshDataRetention()
        }
    }

    fun clearPhotoThumbnailsCache() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            listOf("continuity_camera", "clipboard").forEach { dirName ->
                java.io.File(context.cacheDir, dirName).listFiles()?.forEach { file ->
                    runCatching { file.deleteRecursively() }
                }
            }
            refreshDataRetention()
        }
    }

    fun clearReceivedFileHistory() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            java.io.File(context.cacheDir, "received").listFiles()?.forEach { file ->
                runCatching { file.deleteRecursively() }
            }
            _state.update { s -> s.copy(transfers = s.transfers.filterNot { it.isFinished() }) }
            runCatching { node?.clearReceivedFileHistory() }.onFailure {
                Log.w(TAG, "couldn't clear the received file history", it)
                _messages.tryEmit(context.getString(R.string.data_clear_failed))
            }
            refreshTimeline()
            refreshDataRetention()
        }
    }

    fun clearEverything() {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val failed = runCatching { node?.clearAllLocalData() }
                .onFailure { Log.w(TAG, "couldn't clear local data", it) }
                .isFailure
            listOf("mms", "continuity_camera", "clipboard", "received", PcAppIcons.DIR).forEach { dirName ->
                java.io.File(context.cacheDir, dirName).listFiles()?.forEach { file ->
                    runCatching { file.deleteRecursively() }
                }
            }
            pcAppIcons.evictAll()
            _state.update { s -> s.copy(transfers = s.transfers.filterNot { it.isFinished() }) }
            refreshClipboardHistory()
            refreshTimeline()
            refreshSendQueue()
            refreshDataRetention()
            _messages.tryEmit(
                context.getString(if (failed) R.string.data_clear_failed else R.string.data_cleared_everything_toast),
            )
        }
    }

    fun resendTimelineEntry(entry: TimelineEntry) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            val pcName = _state.value.nameOf(entry.deviceId).orEmpty().ifEmpty { context.getString(R.string.your_pc) }
            val wasOnline = _state.value.device(entry.deviceId)?.online == true
            when (entry.kind) {
                TimelineKind.CLIP -> {
                    val clipId = entry.refId ?: return@launch
                    try {
                        currentNode.resendClipboardHistory(clipId, entry.deviceId)
                        if (wasOnline) {
                            _messages.tryEmit(context.getString(R.string.clip_sent_to, pcName))
                        } else {
                            refreshSendQueue()
                            emitOfflineQueuedNotice(entry.deviceId, pcName)
                        }
                    } catch (e: G7LinkException) {
                        _messages.tryEmit(describe(e))
                    }
                }
                TimelineKind.LINK -> {
                    val url = entry.target.ifEmpty { entry.title }
                    val msg = openLinkOnPc(entry.deviceId, url)
                    if (wasOnline) {
                        _messages.tryEmit(msg)
                    }
                }
                TimelineKind.FILE, TimelineKind.PHOTO -> {
                    val target = entry.target
                    when {
                        target.startsWith("content://") -> sendFiles(entry.deviceId, listOf(Uri.parse(target)))
                        target.isNotEmpty() && java.io.File(target).exists() -> {
                            val f = java.io.File(target)
                            send(
                                entry.deviceId,
                                listOf(
                                    FileToSend.Path(
                                        path = f.absolutePath,
                                        name = f.name,
                                        folder = null,
                                    ),
                                ),
                            )
                        }
                    }
                }
                TimelineKind.RECORDING -> {
                    val target = entry.target
                    if (target.isNotEmpty() && java.io.File(target).exists()) {
                        val f = java.io.File(target)
                        try {
                            currentNode.sendRecording(
                                entry.deviceId,
                                FileToSend.Path(path = f.absolutePath, name = f.name, folder = null),
                                emptyList(),
                            )
                        } catch (e: G7LinkException) {
                            _messages.tryEmit(describe(e))
                        }
                    }
                }
                TimelineKind.SESSION -> Unit
            }
        }
    }

    /**
     * Keeps the paired PCs' beacon keys where the background scan's receiver
     * finds them, and the scan running while there's a PC to look for.
     */
    private fun syncBeaconKeys(node: G7LinkNode) {
        val keys = runCatching { node.bleBeaconKeys() }.getOrNull() ?: return
        app.g7link.android.companion.BeaconKeys.replace(context, keys.associate { it.id to it.key })
        app.g7link.android.companion.PcBeaconScan.sync(context)
    }

    /** What this phone offers PCs (docs/protocol/capabilities.md). */
    private fun capabilities(notificationAccess: Boolean): List<String> =
        CLIPBOARD_CAPABILITIES +
            // Recognises a paired PC's rotating Bluetooth beacon.
            (if (app.g7link.android.companion.PcBeaconScan.canScan(context)) listOf("ble.beacon2") else emptyList()) +
            // Opens a Bluetooth link to a PC when there's no shared network, and carries the touchpad over it by choice.
            (if (PcBleLink.canConnect(context)) listOf("ble.link", "ble.remote") else emptyList()) +
            // Play on phone: shows a PC's playback, streamed from the PC.
            listOf("media.watch") +
            // Shows a paired phone's or tablet's notifications.
            listOf("notify.show") +
            // Follows and answers a PC's coding-agent threads (docs/protocol/agents.md),
            // and shows the sessions started on it outside G7 when the PC allows (§3).
            listOf("agent.remote", "agent.follow") +
            // Phone as microphone: asks for the permission when a PC first wants it.
            (if (context.packageManager.hasSystemFeature(PackageManager.FEATURE_MICROPHONE)) listOf("mic.stream") else emptyList()) +
            toggles.capabilities() +
            (if (notificationAccess) NOTIFICATION_CAPABILITIES + MEDIA_CAPABILITIES else emptyList()) +
            (if (notificationAccess && NotificationListener.canReadSensitive(context)) listOf("notify.sensitive") else emptyList()) +
            (if (_state.value.photoAccess) PHOTO_CAPABILITIES else emptyList()) +
            (if (PhoneCalls.canFollow(context)) listOf("call.state") else emptyList()) +
            (if (PhoneCalls.canControl(context)) listOf("call.control") else emptyList()) +
            (if (PhoneCalls.canControl(context) && CallCompanion.allowed(context)) listOf("call.incall") else emptyList()) +
            (if (PhoneCalls.canReadLog(context)) listOf("call.log") else emptyList()) +
            (if (PhoneCalls.canDial(context)) listOf("call.dial") else emptyList()) +
            (if (PhoneCalls.canListSims(context)) listOf("call.sims") else emptyList()) +
            (if (PhoneContacts.canRead(context)) listOf("contacts.read") else emptyList()) +
            (if (PhoneSms.canRead(context)) listOf("sms.read") else emptyList()) +
            (if (PhoneSms.canSend(context)) listOf("sms.send") else emptyList()) +
            PhoneStorage.capabilities(context) +
            (if (WebcamService.hasPermission(context)) listOf("camera.stream") else emptyList()) +
            (if (context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) listOf("camera.capture") else emptyList()) +
            (if (InputService.running || Elevated.running) listOf("mirror.input") else emptyList()) +
            (if (Elevated.running) listOf("clip.read.auto") else emptyList()) +
            // Apps in windows of their own run on displays the Elevated helper makes.
            (if (Elevated.running && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) listOf("mirror.virtual_display") else emptyList()) +
            // Asked for when a PC wants the sound, or streamed via Elevated helper.
            (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q || Elevated.running) listOf("mirror.audio.playback") else emptyList()) +
            // The same sound on its own, without the screen.
            (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q || Elevated.running) listOf("mirror.sound") else emptyList()) +
            (if (app.g7link.android.mirror.ScreenEncoder.supportsHevcDecoder()) listOf("mirror.decode.hevc") else emptyList())

    /**
     * The phone was unlocked: redial a paired PC that's offline now instead
     * of at its next retry, which can be up to two minutes off.
     */
    fun reconnectIfOffline() {
        if (_state.value.devices.none { !it.online }) return
        scope.launch { runCatching { node?.networkChanged() } }
    }

    /**
     * Triggers an immediate network/presence reconnect (e.g. when woken by
     * CompanionDeviceService on BLE proximity beacon appearance).
     */
    fun reconnectNow() {
        start()
        scope.launch {
            node?.networkChanged()
            if (!Elevated.running) Elevated.start()
        }
    }

    /**
     * The PC [pcId], whose beacon asked for it from [address], gets a Bluetooth
     * link once the core is up, if the user lets this phone use Bluetooth with
     * it. [done] runs when that's settled.
     */
    fun connectOverBluetooth(pcId: String, address: String, done: () -> Unit) {
        start()
        scope.launch {
            try {
                withTimeoutOrNull(BLE_START_WAIT_MS) { state.first { it.status is CoreStatus.Ready } }
                val current = node ?: return@launch
                if (!app.g7link.android.ble.PcBleLink.canConnect(context)) return@launch
                app.g7link.android.ble.PcBleLink.connect(context, current, pcId, address)
            } finally {
                done()
            }
        }
    }

    /**
     * Lets this phone and [pcId] use Bluetooth when they share no network: ring,
     * the hotspot and small copied text (off by default). Turning it on needs
     * the Nearby devices permission, asked for by the switch.
     */
    fun setBleLink(pcId: String, on: Boolean) {
        _state.update { it.withBleLink(pcId, on) }
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            applyBleLink(currentNode, pcId, on)
        }
    }

    /** Sets this phone's Bluetooth toggle for [pcId] in the core, and tells the PCs what this phone offers now. */
    private suspend fun applyBleLink(node: G7LinkNode, pcId: String, on: Boolean) {
        runCatching { node.setDeviceToggle(pcId, BLE_LINK_TOGGLE, on) }
            .onFailure { Log.w(TAG, "can't change the Bluetooth link toggle", it) }
        node.updatePower(powerLevel(), capabilities(_state.value.notificationAccess))
    }

    // ---- The touchpad's connection: Auto, the network or Bluetooth ----

    /** Each PC's Bluetooth bring-up for the touchpad ([setUpBluetooth]). */
    private val bleSetups = java.util.concurrent.ConcurrentHashMap<String, Job>()

    /** The Remote screen's hold on each PC's link, renewed while it shows ([remoteShown]). */
    private val bleHolds = java.util.concurrent.ConcurrentHashMap<String, Job>()

    /** When each PC was last asked to keep a link. */
    private val bleHoldAsked = java.util.concurrent.ConcurrentHashMap<String, Long>()

    /** How often background renewals look for each PC's window, after tries that came to nothing. */
    private val bleBackoffs = java.util.concurrent.ConcurrentHashMap<String, BleBackoff>()

    /** PCs that keep a link for this phone until it lets go (or their lease ends). */
    private val bleHeld: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    /** PCs that said yes to Bluetooth since the core started: with no network, their window can still be found. */
    private val bleAllowedBy: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    /** Tells the core whether each PC's touchpad shows, newest only. */
    private val remoteActive = LatestValueSender<String, Boolean>(
        scope = scope,
        onError = { Log.w(TAG, "the touchpad's state didn't reach the core", it) },
    ) send@{ id, on ->
        startJob?.join()
        val current = node ?: return@send
        current.setRemoteActive(id, on)
    }

    /**
     * The touchpad's way to [pcId] (§2.3). Kept on this phone, and given to the
     * core between input ops so no gesture is split across paths. Network lets
     * the Bluetooth hold go; Auto brings Bluetooth up quietly where it's allowed.
     */
    fun setRemoteRoute(pcId: String, route: RemoteRoute) {
        app.g7link.android.ui.Preferences.setRemoteRoute(context, pcId, route)
        _state.update { it.withRoute(pcId, route) }
        enqueueRemoteOp { node ->
            runCatching { node.setRemoteRoute(pcId, route) }
                .onFailure { Log.w(TAG, "the touchpad's route didn't reach the core", it) }
            runCatching { node.remoteRouteState(pcId) }.getOrNull()?.let { routeState ->
                _state.update { it.withRouteState(pcId, routeState) }
            }
        }
        when (route) {
            RemoteRoute.NETWORK -> {
                bleSetups.remove(pcId)?.cancel()
                releaseBleHold(pcId)
            }
            RemoteRoute.AUTO -> if (mayHoldBle(pcId)) setUpBluetooth(pcId, ask = false)
            RemoteRoute.BLUETOOTH -> Unit
        }
    }

    /**
     * Brings up Bluetooth with [pcId] for the touchpad ([useBluetooth]), shown in
     * [Device.bleSetup] as it goes. Another call starts it over.
     */
    fun setUpBluetooth(pcId: String, ask: Boolean) {
        val runs = runScope ?: return
        bleSetups.remove(pcId)?.cancel()
        bleBackoff(pcId).reset()
        val job = runs.launch(Dispatchers.IO) {
            useBluetooth(pcId, ask).collect { step -> _state.update { it.withBleSetup(pcId, step) } }
        }
        bleSetups[pcId] = job
        job.invokeOnCompletion { bleSetups.remove(pcId, job) }
    }

    /**
     * Bluetooth with [pcId] for the touchpad, step by step (§2.3, steps 4 to 6).
     * With [ask] the user asked for it: this phone's switch for that PC goes on
     * (picking Bluetooth is the consent) and the PC may ask its user. Without, only
     * what's allowed already is used and nobody is asked. Nothing happens without
     * the Nearby devices permission and Bluetooth on: the sheet sees to those first.
     */
    fun useBluetooth(pcId: String, ask: Boolean): Flow<BleSetup> = flow {
        startJob?.join()
        val current = node
        if (current == null) {
            emit(BleSetup.NoPath)
            return@flow
        }
        if (!PcBleLink.canConnect(context) || !PcBleLink.bluetoothOn(context)) return@flow
        if (ask) {
            _state.update { it.withBleLink(pcId, true) }
            applyBleLink(current, pcId, true)
        } else if (_state.value.device(pcId)?.bleLink != true) {
            return@flow
        }
        val device = _state.value.device(pcId) ?: return@flow
        emit(BleSetup.Starting)
        if (!device.online && !device.bleLinked) {
            // Nothing to ask over. A PC that said yes opens a window by itself when its session ends.
            if (pcId in bleAllowedBy) {
                emit(BleSetup.Connecting)
                emit(afterLinkTry(pcId, linkNow(current, pcId, BLE_NO_NETWORK_SCAN_MS)))
            } else {
                emit(BleSetup.NoPath)
            }
            return@flow
        }
        val askedAt = SystemClock.elapsedRealtime()
        var reply = bleUseOnce(current, pcId, ask)
        if (ask && reply.getOrNull()?.state == BleUseState.UNAVAILABLE) {
            // The PC may not have heard yet that this phone offers Bluetooth now.
            delay(BLE_CAPABILITIES_SETTLE_MS)
            reply = bleUseOnce(current, pcId, ask)
        }
        while (reply.getOrNull()?.state == BleUseState.ASKING && SystemClock.elapsedRealtime() - askedAt < BLE_ASK_WAIT_MS) {
            emit(BleSetup.Asking)
            delay(BLE_ASK_POLL_MS)
            reply = bleUseOnce(current, pcId, ask = false)
        }
        val answer = reply.getOrNull()
        when {
            answer == null -> emit(bleSetupAfterError(reply.exceptionOrNull()))
            answer.state != BleUseState.OPEN -> emit(bleSetupAfter(answer.state, asked = ask))
            // Open while a link is already up counts as linked.
            PcBleLink.linkedTo(pcId) -> emit(BleSetup.Linked)
            // The PC opened its windows for this phone lately: nothing to look for until it can again.
            !answer.windowOpen -> emit(BleSetup.Later)
            else -> {
                emit(BleSetup.Connecting)
                emit(afterLinkTry(pcId, linkNow(current, pcId, BLE_CONNECT_WAIT_MS)))
            }
        }
    }

    /** Where a `ble.use` that went wrong leaves the bring-up. */
    private fun bleSetupAfterError(error: Throwable?): BleSetup = when (error) {
        is G7LinkException.Offline -> BleSetup.NoPath
        // An older PC: it doesn't know ble.use.
        is G7LinkException.Unsupported -> BleSetup.Unavailable
        else -> BleSetup.Failed
    }

    /** One `ble.use { on: true }` to [pcId], and what it says about the PC. */
    private suspend fun bleUseOnce(node: G7LinkNode, pcId: String, ask: Boolean): Result<BleUseAnswer> {
        val answer = try {
            node.bleUseReply(pcId, true, ask).let {
                BleUseAnswer(it.state, it.windowSecs?.toInt(), it.retrySecs?.toInt())
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: G7LinkException) {
            Log.i(TAG, "a PC didn't answer about Bluetooth: ${e.javaClass.simpleName}")
            return Result.failure(e)
        }
        bleHoldAsked[pcId] = SystemClock.elapsedRealtime()
        when (answer.state) {
            BleUseState.OPEN, BleUseState.LINKED -> {
                bleHeld += pcId
                bleAllowedBy += pcId
            }
            BleUseState.OFF -> bleAllowedBy -= pcId
            else -> Unit
        }
        return Result.success(answer)
    }

    /** How a try at linking ended: [failure] null once a link is up. */
    private class LinkTry(val failure: LinkFailure?)

    /**
     * Looks for [pcId]'s connect window for up to [scanMs] and links with it: null once
     * a link is up, or why none came. An attempt that found the PC is waited for to its
     * end (it has its own limit); one that ends early gets a fresh scan, a few times.
     */
    private suspend fun linkNow(node: G7LinkNode, pcId: String, scanMs: Long): LinkFailure? {
        val tried = withTimeoutOrNull(scanMs + BLE_LINK_FINISH_MS) {
            val scanUntil = SystemClock.elapsedRealtime() + scanMs
            var attempts = 0
            var failure: LinkFailure = LinkFailure.NotSeen
            while (!PcBleLink.linkedTo(pcId)) {
                if (!PcBleLink.active(pcId)) {
                    // The PC was seen and the attempt ended: that says more than a scan that comes up empty later.
                    if (attempts > 0) failure = PcBleLink.lastOutcome(pcId)?.failure ?: failure
                    val left = scanUntil - SystemClock.elapsedRealtime()
                    if (attempts == BLE_LINK_ATTEMPTS || left <= 0) return@withTimeoutOrNull LinkTry(failure)
                    attempts++
                    val device = PcCompanion.findPcToConnect(context, pcId, left)
                        ?: return@withTimeoutOrNull LinkTry(failure)
                    if (!PcBleLink.connect(context, node, pcId, device, urgent = true)) {
                        return@withTimeoutOrNull LinkTry(LinkFailure.Connect(-1))
                    }
                }
                delay(BLE_LINK_POLL_MS)
            }
            LinkTry(null)
        }
        return if (tried == null) LinkFailure.TimedOut else tried.failure
    }

    /** Where a try at linking with [pcId] leaves the bring-up; counted for the background looks' back-off. */
    private fun afterLinkTry(pcId: String, failure: LinkFailure?): BleSetup {
        val backoff = bleBackoff(pcId)
        if (failure == null) {
            backoff.reset()
        } else {
            val wait = backoff.failed(SystemClock.elapsedRealtime())
            Log.i(TAG, "no Bluetooth link with a PC: $failure; background looks wait ${wait / 1000} s")
        }
        return bleSetupAfterLink(failure)
    }

    private fun bleBackoff(pcId: String): BleBackoff = bleBackoffs.computeIfAbsent(pcId) { BleBackoff() }

    /**
     * The Remote screen for [pcId] shows ([shown]) or went to the background.
     * While it shows the core measures the touchpad's paths more often, and the
     * Bluetooth hold is renewed; in the background the hold lasts a little longer
     * (§6.3), for a glance at another app.
     */
    fun remoteShown(pcId: String, shown: Boolean) {
        remoteActive.offer(pcId, shown)
        keepBleHold(pcId, lingerMs = if (shown) null else BLE_HOLD_LINGER_MS)
    }

    /** The Remote screen for [pcId] closed: the hold goes now. */
    fun remoteClosed(pcId: String) {
        remoteActive.offer(pcId, false)
        bleHolds.remove(pcId)?.cancel()
        releaseBleHold(pcId)
    }

    /** Renews [pcId]'s hold while it's wanted, for [lingerMs] more or until cancelled, then lets it go. */
    private fun keepBleHold(pcId: String, lingerMs: Long?) {
        val runs = runScope ?: return
        bleHolds.remove(pcId)?.cancel()
        val until = lingerMs?.let { SystemClock.elapsedRealtime() + it }
        val job = runs.launch(Dispatchers.IO) {
            while (until == null || SystemClock.elapsedRealtime() < until) {
                val current = node
                if (current != null && mayHoldBle(pcId)) {
                    val asked = bleHoldAsked[pcId]
                    if (asked == null || SystemClock.elapsedRealtime() - asked >= BLE_HOLD_RENEW_MS) renewBleHold(current, pcId)
                } else {
                    releaseBleHold(pcId)
                }
                delay(BLE_HOLD_TICK_MS)
            }
            releaseBleHold(pcId)
        }
        bleHolds[pcId] = job
        job.invokeOnCompletion { bleHolds.remove(pcId, job) }
    }

    /**
     * Whether the touchpad keeps a Bluetooth link with [pcId] now: Auto or
     * Bluetooth (never while on Network), set up on this phone, and a way to ask.
     */
    private fun mayHoldBle(pcId: String): Boolean {
        val device = _state.value.device(pcId) ?: return false
        return device.bleRemote && device.bleLink && device.route != RemoteRoute.NETWORK &&
            (device.online || device.bleLinked) &&
            PcBleLink.canConnect(context) && PcBleLink.bluetoothOn(context)
    }

    /**
     * Renews [pcId]'s hold. When the PC has its window open and no link is up, looks
     * for it a short while, less often after tries that came to nothing. Only a
     * bring-up the user started shows progress: a renewal changes the row once it
     * knows how it went, and leaves it as it was when it doesn't look.
     */
    private suspend fun renewBleHold(node: G7LinkNode, pcId: String) {
        // A bring-up under way asks for itself.
        if (bleSetups[pcId]?.isActive == true) return
        val answer = bleUseOnce(node, pcId, ask = false).getOrNull() ?: return
        val step = when {
            answer.state != BleUseState.OPEN -> bleSetupAfter(answer.state, asked = false)
            PcBleLink.linkedTo(pcId) -> BleSetup.Linked.also { bleBackoff(pcId).reset() }
            // One under way (the PC's beacon asked for it) is looked at by the next renewal.
            PcBleLink.active(pcId) -> return
            // No window now: nothing to look for until the PC can open one again.
            !answer.windowOpen -> return
            !bleBackoff(pcId).due(SystemClock.elapsedRealtime()) -> return
            else -> afterLinkTry(pcId, linkNow(node, pcId, BLE_HOLD_LINK_MS))
        }
        _state.update { it.withBleSetup(pcId, step) }
    }

    /** Lets [pcId]'s hold go (`ble.use { on: false }`), when this phone has one. */
    private fun releaseBleHold(pcId: String) {
        bleHoldAsked.remove(pcId)
        // The next hold looks for the PC from the start.
        bleBackoffs.remove(pcId)
        if (!bleHeld.remove(pcId)) return
        val current = node ?: return
        scope.launch(Dispatchers.IO) {
            try {
                current.bleUse(pcId, false, false)
            } catch (e: G7LinkException) {
                Log.i(TAG, "a PC wasn't told to let its Bluetooth link go: ${e.javaClass.simpleName}")
            }
        }
    }

    /** Drops what the touchpad kept about [pcId]'s Bluetooth: it was unpaired. */
    private fun forgetBle(pcId: String) {
        bleSetups.remove(pcId)?.cancel()
        bleHolds.remove(pcId)?.cancel()
        bleHoldAsked.remove(pcId)
        bleBackoffs.remove(pcId)
        bleHeld.remove(pcId)
        bleAllowedBy.remove(pcId)
    }

    /**
     * Elevated while the wireless debugging helper runs, Assist when the
     * user turned on control from the PC (docs/PLAN.md §4.6).
     */
    private fun powerLevel(): PowerLevel = when {
        Elevated.running -> PowerLevel.ELEVATED
        InputService.running -> PowerLevel.ASSIST
        else -> PowerLevel.BASIC
    }

    // ---- Pairing ----

    /**
     * Pairs with a link another app opened (a browser, a chat, the system camera), once the
     * core has started (opening the link may have launched the app). Anyone can send such a
     * link, with a secret of their own, so its secret isn't trusted: both devices show a fresh
     * 6-digit code to compare, as in nearby pairing, and it arrives as a pairing event.
     */
    fun joinFromOtherApp(link: String) = command {
        _state.update { it.copy(pairing = PairingState.Joining) }
        try {
            it.pairingJoinLink(link)
        } catch (e: G7LinkException) {
            val failure = when (e) {
                is G7LinkException.InvalidPairingLink -> PairingFailure.Other("not a G7 Link code")
                is G7LinkException.Denied -> PairingFailure.Rejected
                else -> PairingFailure.Unreachable
            }
            // Past the code, the core reports how it ended itself.
            _state.update { s ->
                if (s.pairing == PairingState.Joining) s.copy(pairing = PairingState.Failed(failure)) else s
            }
        }
    }

    /** Pairs with the PC whose QR code was scanned. */
    fun joinWithLink(link: String) = command {
        _state.update { it.copy(pairing = PairingState.Joining) }
        try {
            it.pairingJoin(link)
        } catch (e: G7LinkException) {
            val failure = when (e) {
                is G7LinkException.InvalidPairingLink -> PairingFailure.Other("not a G7 Link code")
                is G7LinkException.Denied -> PairingFailure.Rejected
                else -> PairingFailure.Unreachable
            }
            _state.update { s -> s.copy(pairing = PairingState.Failed(failure)) }
        }
    }

    /**
     * A pairing link for another phone or tablet to scan (this device hosts the pairing,
     * as a PC does); null if the core can't make one now.
     */
    suspend fun pairingLink(): String? {
        startJob?.join()
        val node = node ?: return null
        return try {
            node.pairingStartQr()
        } catch (e: G7LinkException) {
            _messages.tryEmit(describe(e))
            null
        }
    }

    /** Pairs with a nearby PC that shows its pairing screen. */
    fun pairNearby(peer: String) = command {
        _state.update { it.copy(pairing = PairingState.Connecting(peer)) }
        try {
            it.pairingStartNearby(peer)
        } catch (e: G7LinkException) {
            _state.update { s ->
                if (s.pairing is PairingState.Connecting) s.copy(pairing = PairingState.Failed(PairingFailure.Unreachable)) else s
            }
        }
    }

    fun confirmCode(matches: Boolean) {
        val comparing = _state.value.pairing as? PairingState.Comparing ?: return
        _state.update {
            it.copy(
                pairing = if (matches) PairingState.Confirmed(comparing.peer, comparing.code)
                else PairingState.Failed(PairingFailure.Declined),
            )
        }
        scope.launch(Dispatchers.IO) {
            // The result comes as an event; with no answer sent there won't be one.
            val sent = runCatching { node?.pairingConfirm(matches) != null }.getOrDefault(false)
            if (matches && !sent) {
                val failed = PairingState.Failed(PairingFailure.Other("the answer didn't reach the core"))
                _state.update { s -> if (s.pairing is PairingState.Confirmed) s.copy(pairing = failed) else s }
            }
        }
    }

    /**
     * Keeps local discovery working while the pairing screen is open, and has
     * it ask the network who's there often only then: asking all the time
     * kept the phone awake.
     */
    fun setPairingScreenOpen(open: Boolean) {
        pairingMulticast?.runCatching { if (open) acquire() else release() }
        scope.launch(discoveryPace) { runCatching { node?.setDiscoveryFast(open) } }
    }

    /** One at a time, so a quick open and close can't land out of order. */
    private val discoveryPace = Dispatchers.IO.limitedParallelism(1)

    fun resetPairing() {
        _state.update { it.copy(pairing = PairingState.Idle) }
        scope.launch(Dispatchers.IO) { runCatching { node?.pairingCancel() } }
    }

    // ---- Devices ----

    fun ring(id: String, on: Boolean) = command { it.ring(id, on) }

    /** Tells paired PCs that `version` is out on `channel`, so they needn't wait for their next check. */
    fun announceUpdate(channel: String, version: String) = command { it.announceUpdate(channel, version) }

    /** Locks a PC, or puts it to sleep; says how it went, and calls [onDone] on the main thread with whether it did. */
    fun pcPower(id: String, sleep: Boolean, onDone: (Boolean) -> Unit = {}) {
        scope.launch {
            var done = false
            try {
                startJob?.join()
                val node = node
                val name = _state.value.nameOf(id).orEmpty()
                val message = if (node == null) {
                    context.getString(R.string.error_not_connected)
                } else {
                    try {
                        node.pcPower(id, sleep)
                        done = true
                        context.getString(if (sleep) R.string.pc_sleeping else R.string.pc_locked, name)
                    } catch (e: G7LinkException) {
                        if (e is G7LinkException.Denied) context.getString(R.string.pc_actions_off, name) else describe(e)
                    }
                }
                _messages.tryEmit(message)
            } finally {
                withContext(NonCancellable + Dispatchers.Main) { onDone(done) }
            }
        }
    }

    /**
     * Sends Wake-on-LAN magic packets to a sleeping or shut-down PC and waits
     * up to ~60 s for it to connect before showing troubleshooting tips.
     */
    fun wake(id: String) {
        wakeTimers.remove(id)?.cancel()
        _state.update { it.withWakeState(id, WakeState.Waking) }
        val timer = scope.launch(Dispatchers.IO) {
            startJob?.join()
            val node = node ?: return@launch
            repeat(15) {
                delay(4_000)
                val dev = _state.value.device(id)
                if (dev == null || dev.online || dev.wakeState != WakeState.Waking) return@launch
                runCatching { node.refresh() }
            }
            _state.update { s ->
                val dev = s.device(id)
                if (dev != null && !dev.online && dev.wakeState == WakeState.Waking) {
                    s.withWakeState(id, WakeState.TimedOut)
                } else {
                    s
                }
            }
        }
        wakeTimers[id] = timer
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val node = node ?: return@launch
            try {
                node.wake(id)
                node.refresh()
            } catch (e: G7LinkException) {
                wakeTimers.remove(id)?.cancel()
                _state.update { it.withWakeState(id, WakeState.Idle) }
                val name = _state.value.nameOf(id).orEmpty()
                val msg = when (e) {
                    is G7LinkException.Unsupported -> context.getString(R.string.wake_not_ready, name)
                    else -> describe(e)
                }
                _messages.tryEmit(msg)
            }
        }
    }

    /**
     * Opens a link, video with timestamp, or map location on a PC; returns what to tell the user.
     * The PC asks its user before opening it, unless [userShared]: pass true only when the user
     * shared this link from the system share sheet.
     */
    suspend fun openLinkOnPc(id: String, url: String, userShared: Boolean = false): String {
        startJob?.join()
        val node = node ?: return context.getString(R.string.clip_no_pc)
        val name = _state.value.nameOf(id).orEmpty()
        val wasOnline = _state.value.device(id)?.online == true
        val handoff = app.g7link.core.extractHandoffLink(url)
        val finalUrl = if (
            handoff != null &&
            handoff.kind == app.g7link.core.HandoffKind.VIDEO_LINK &&
            handoff.timestampSecs == null
        ) {
            val activeSecs = phoneMedia.activePositionSeconds()
            if (activeSecs != null) app.g7link.core.withVideoTimestamp(handoff.url, activeSecs) else handoff.url
        } else {
            handoff?.url ?: url
        }
        return try {
            if (userShared) node.openSharedLink(id, finalUrl) else node.openLink(id, finalUrl)
            if (wasOnline) {
                context.getString(R.string.link_opened_on, name)
            } else {
                refreshSendQueue()
                val qid = runCatching { node.latestQueuedIdForPeer(id) }.getOrNull()
                val displayName = name.ifBlank { context.getString(R.string.your_pc) }
                val msg = context.getString(R.string.send_queue_offline_toast, displayName)
                _queuedNotices.tryEmit(
                    OfflineQueuedNotice(
                        message = msg,
                        queueId = qid,
                        peerId = id,
                    ),
                )
                msg
            }
        } catch (e: G7LinkException) {
            describe(e)
        }
    }

    // ---- Remote input (phone -> PC) ----

    /** Checks whether a PC accepts mouse, keyboard and presentation input from this phone. */
    suspend fun remoteCheck(id: String): RemoteAccess {
        startJob?.join()
        val node = node ?: return RemoteAccess.Offline
        return try {
            node.remoteCheck(id)
            RemoteAccess.Allowed
        } catch (e: G7LinkException) {
            when (e) {
                is G7LinkException.Denied -> RemoteAccess.Denied
                is G7LinkException.Unsupported -> RemoteAccess.Unsupported
                else -> RemoteAccess.Offline
            }
        }
    }

    private val pendingMoves = DeltaCoalescer(limit = MAX_MOVE_DELTA)
    private val pendingScrolls = DeltaCoalescer(limit = MAX_SCROLL_DELTA)

    /** Moves a PC's cursor by `(dx, dy)` logical pixels over QUIC datagrams, coalescing rapid 120Hz touch samples. */
    fun remoteMove(id: String, dx: Float, dy: Float) {
        if (!pendingMoves.add(id, dx, dy)) return
        val queued = enqueuePointerOp { node ->
            pendingMoves.take()?.let { runCatching { node.remoteMove(it.target, it.dx, it.dy) } }
        }
        if (!queued) pendingMoves.clear()
    }

    /**
     * Scrolls on a PC (`dy` > 0 scrolls down, `dx` > 0 scrolls right, in wheel notches).
     * Smooth ([fast]) scrolls are datagrams, summed until sent; wheel clicks keep their order.
     */
    fun remoteScroll(id: String, dx: Float, dy: Float, fast: Boolean = true) {
        if (!fast) {
            enqueueRemoteOp { runCatching { it.remoteScroll(id, dx, dy, false) } }
            return
        }
        if (!pendingScrolls.add(id, dx, dy)) return
        val queued = enqueuePointerOp { node ->
            pendingScrolls.take()?.let { runCatching { node.remoteScroll(it.target, it.dx, it.dy, true) } }
        }
        if (!queued) pendingScrolls.clear()
    }

    /** Presses, releases or clicks a mouse button (`"left"`, `"right"` or `"middle"`). */
    fun remoteButton(id: String, button: String, action: String, onStatus: ((RemoteAccess) -> Unit)? = null) {
        if (action != "up") _pcCaretMoved.tryEmit(id)
        remoteCommand(onStatus) { it.remoteButton(id, button, action) }
    }

    /** Types Unicode text on a PC (dictation), splitting long strings into 256-byte chunks. */
    fun remoteText(id: String, text: String, onStatus: ((RemoteAccess) -> Unit)? = null) {
        _pcCaretMoved.tryEmit(id)
        remoteCommand(onStatus) { node ->
            for (chunk in app.g7link.android.ui.remote.UtteranceJoiner.chunkUtf8(text)) {
                node.remoteText(id, chunk)
            }
        }
    }

    /** Presses a named key or shortcut with optional modifiers on a PC. */
    fun remoteKey(id: String, key: String, mods: List<String> = emptyList(), onStatus: ((RemoteAccess) -> Unit)? = null) {
        if (key !in KEYS_KEEPING_CARET) _pcCaretMoved.tryEmit(id)
        remoteCommand(onStatus) { it.remoteKey(id, key, mods) }
    }

    /**
     * PCs whose caret or focus this phone just moved (a click, a key such as
     * an arrow, Enter or a paste): in a field it can't read, what the typing
     * box holds may no longer match.
     */
    val pcCaretMoved: SharedFlow<String> get() = _pcCaretMoved
    private val _pcCaretMoved = MutableSharedFlow<String>(extraBufferCapacity = 8)

    /** Whether PC [id] mirrors its focused text field and takes [remoteTyping] edits by range. */
    fun remoteMirrorsText(id: String): Boolean = runCatching { node?.remoteMirrorsText(id) == true }.getOrDefault(false)

    /**
     * Tells PC [id] the typing box opened ([on]) or closed: while open, the PC
     * sends its focused field ([CoreState.pcTextState]).
     */
    fun remoteTextWatch(id: String, on: Boolean) {
        if (!on) _state.update { it.copy(pcTextState = it.pcTextState - id) }
        enqueueRemoteOp { runCatching { it.remoteTextWatch(id, on) } }
    }

    /** App icons from the PCs, by "pcId/key": a key names one icon, so a changed icon has a new key. */
    private val pcAppIcons = LruCache<String, Bitmap>(PcAppIcons.IN_MEMORY)

    /**
     * The icon of an app on PC [pcId] that its field state named by [key]:
     * from memory, from this phone's cache or from the PC. Null when there's
     * none to be had; never throws.
     */
    suspend fun pcAppIcon(pcId: String, key: String): Bitmap? {
        if (!PcAppIcons.isKey(key) || !PcAppIcons.isPcFolder(pcId)) return null
        val name = "$pcId/$key"
        pcAppIcons.get(name)?.let { return it }
        val icon = withContext(Dispatchers.IO) {
            val file = java.io.File(context.cacheDir, "${PcAppIcons.DIR}/$name.png")
            PcAppIcons.read(file) ?: fetchPcAppIcon(pcId, key)?.let { bytes ->
                PcAppIcons.decode(bytes)?.also { PcAppIcons.save(file, bytes) }
            }
        } ?: return null
        pcAppIcons.put(name, icon)
        return icon
    }

    /** An app icon's bytes as PC [pcId] sent them: not yet checked. */
    private suspend fun fetchPcAppIcon(pcId: String, key: String): ByteArray? {
        startJob?.join()
        val node = node ?: return null
        return try {
            node.remoteAppIcon(pcId, key)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.i(TAG, "an app icon wasn't fetched: ${e.javaClass.simpleName}")
            null
        }
    }

    /**
     * Sends one change of the typing box to PC [id], in order with keys and
     * clicks; [onDone] hears how it went, on the main thread. A PC that
     * doesn't take edits gets the same as arrow keys, Backspaces and text.
     */
    internal fun remoteTyping(
        id: String,
        op: app.g7link.android.ui.remote.TypingOp,
        onStatus: ((RemoteAccess) -> Unit)?,
        onDone: (app.g7link.android.ui.remote.TypingResult) -> Unit,
    ) {
        val send: suspend (G7LinkNode) -> Unit = { started ->
            // Whatever goes wrong, the box hears: it holds the next change until then.
            val result = try {
                runTyping(started, id, op, onStatus)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "typing failed: ${e.javaClass.simpleName}")
                app.g7link.android.ui.remote.TypingResult.Failed
            }
            withContext(Dispatchers.Main) { onDone(result) }
        }
        val queued = synchronized(opsLock) { isRunning && node != null && remoteOps.trySend(send).isSuccess }
        if (!queued) onDone(app.g7link.android.ui.remote.TypingResult.Failed)
    }

    private suspend fun runTyping(
        node: G7LinkNode,
        id: String,
        op: app.g7link.android.ui.remote.TypingOp,
        onStatus: ((RemoteAccess) -> Unit)?,
    ): app.g7link.android.ui.remote.TypingResult {
        val focus = op.focus.toUInt()
        val seq = op.seq.toUInt()
        return try {
            when (op) {
                is app.g7link.android.ui.remote.TypingOp.Caret ->
                    node.remoteCaret(id, focus, seq, op.caret.toUInt(), op.anchor.toUInt())
                is app.g7link.android.ui.remote.TypingOp.Edit ->
                    if (node.remoteMirrorsText(id)) {
                        node.remoteEdit(
                            id,
                            focus,
                            seq,
                            op.rangeStart?.toUInt(),
                            op.rangeEnd?.toUInt(),
                            op.moves,
                            op.backspaces.toUShort(),
                            op.text,
                        )
                    } else {
                        // An older PC: the same, a key at a time.
                        val arrow = if (op.moves < 0) "left" else "right"
                        repeat(kotlin.math.abs(op.moves)) { node.remoteKey(id, arrow, emptyList()) }
                        repeat(op.backspaces) { node.remoteKey(id, "backspace", emptyList()) }
                        op.text.split('\n').forEachIndexed { i, line ->
                            if (i > 0) node.remoteKey(id, "enter", listOf("shift"))
                            for (chunk in app.g7link.android.ui.remote.UtteranceJoiner.chunkUtf8(line)) {
                                node.remoteText(id, chunk)
                            }
                        }
                    }
            }
            onStatus?.invoke(RemoteAccess.Allowed)
            app.g7link.android.ui.remote.TypingResult.Done
        } catch (e: G7LinkException) {
            when (e) {
                is G7LinkException.Denied -> onStatus?.invoke(RemoteAccess.Denied)
                is G7LinkException.Unsupported -> onStatus?.invoke(RemoteAccess.Unsupported)
                is G7LinkException.Offline, is G7LinkException.Timeout, is G7LinkException.Network ->
                    onStatus?.invoke(RemoteAccess.Offline)
                else -> {}
            }
            val stale = e is G7LinkException.Internal && e.reason.endsWith("stale focus")
            if (stale) app.g7link.android.ui.remote.TypingResult.StaleFocus else app.g7link.android.ui.remote.TypingResult.Failed
        }
    }

    /** Sends a presentation slide command (`"next"`, `"previous"`, `"start"`, `"stop"`, `"black"`). */
    fun remoteSlide(id: String, action: String, onStatus: ((RemoteAccess) -> Unit)? = null) =
        remoteCommand(onStatus) { it.remoteSlide(id, action) }

    /** Moves or hides the laser pointer on a PC (`x`, `y` in `0..1`). */
    fun remoteLaser(id: String, on: Boolean, x: Float = 0.5f, y: Float = 0.5f) {
        enqueueRemoteOp { runCatching { it.remoteLaser(id, on, x, y) } }
    }

    /**
     * Asks a paired PC to start streaming its desktop screen to this phone (`mirror.start`,
     * session 0), or with [watch], what that PC player plays, with its sound (Play on phone).
     */
    fun startPcMirror(
        id: String,
        maxSize: Int = 1280,
        fps: Int = 30,
        bitrate: Int = 4_000_000,
        watch: String? = null,
    ) {
        command {
            it.startPcMirror(
                id,
                maxSize.coerceIn(320, 1920).toUInt(),
                fps.coerceIn(10, 60).toUInt(),
                bitrate.coerceIn(500_000, 20_000_000).toUInt(),
                watch,
            )
        }
    }

    /** Stops an active PC-to-phone screen mirror session (`mirror.stop`, session 0). */
    fun stopPcMirror(id: String) {
        if (_pcMirrorConfig.value?.pcId == id) {
            _pcMirrorConfig.value = null
        }
        if (lastPcMirrorKeyframe?.pcId == id) {
            lastPcMirrorKeyframe = null
        }
        scope.launch(Dispatchers.IO) {
            runCatching { node?.mirrorStop(id, 0u) }
        }
    }

    /** Requests an immediate IDR keyframe from a paired PC streaming its screen. */
    fun requestPcMirrorKeyframe(id: String) {
        enqueueRemoteOp { runCatching { it.requestPcMirrorKeyframe(id) } }
    }

    /** Sends a normalized touch, scroll, key, or text event to a paired PC during reverse mirroring. */
    fun sendPcMirrorInput(id: String, input: app.g7link.core.MirrorInputEvent) {
        enqueueRemoteOp { node ->
            runCatching { node.sendPcMirrorInput(id, input) }
                .onFailure { android.util.Log.w("G7Link", "screen input not sent", it) }
        }
    }

    private fun remoteCommand(onStatus: ((RemoteAccess) -> Unit)?, block: suspend (G7LinkNode) -> Unit) {
        enqueueRemoteOp { node ->
            try {
                block(node)
                onStatus?.invoke(RemoteAccess.Allowed)
            } catch (e: G7LinkException) {
                val status = when (e) {
                    is G7LinkException.Denied -> RemoteAccess.Denied
                    is G7LinkException.Unsupported -> RemoteAccess.Unsupported
                    else -> RemoteAccess.Offline
                }
                onStatus?.invoke(status)
            }
        }
    }

    // ---- Agents (docs/protocol/agents.md) ----

    private val _agents = MutableStateFlow<Map<String, AgentsState>>(emptyMap())

    /**
     * Each PC's agent threads and the sessions started on it outside G7, by PC,
     * in memory only. Kept apart from [state] so streaming text redraws only
     * the thread on screen.
     */
    val agents: StateFlow<Map<String, AgentsState>> = _agents.asStateFlow()

    /** The thread whose screen is in front (PC, thread): its notifications wait. */
    @Volatile private var agentForeground: Pair<String, String>? = null

    /** The followed session whose view is in front (PC, session). */
    @Volatile private var agentSessionForeground: Pair<String, String>? = null

    /** Threads this phone started, opened or replied to ("pc/thread"): their working notification shows. */
    private val agentFollowed: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    private val _agentKeptSessions = MutableStateFlow<Set<String>>(emptySet())

    /**
     * Sessions the user keeps in the status bar ("pc/session"), in memory only:
     * a Live Update while they work, until Stop following, until they end or go,
     * or until the PC goes away.
     */
    val agentKeptSessions: StateFlow<Set<String>> = _agentKeptSessions.asStateFlow()

    /** The thread each PC should send this phone's text for; absent for none. */
    private val agentWatchWanted = java.util.concurrent.ConcurrentHashMap<String, String>()
    private val agentWatchLock = Mutex()
    private val agentRereads = java.util.concurrent.ConcurrentHashMap<String, Job>()

    /** The session each PC should send this phone's items for; absent for none. */
    private val agentSessionWatchWanted = java.util.concurrent.ConcurrentHashMap<String, String>()
    private val agentSessionWatchLock = Mutex()

    /** Whether a sessions request may go out unasked (it would make the PC prompt otherwise). */
    private val agentSessionsConsent = app.g7link.android.ui.agents.SessionsConsent(
        object : app.g7link.android.ui.agents.SessionsConsent.Store {
            override fun allowedBefore(pcId: String): Boolean = app.g7link.android.ui.Preferences.agentSessionsAllowed(context, pcId)

            override fun setAllowedBefore(pcId: String, allowed: Boolean) =
                app.g7link.android.ui.Preferences.setAgentSessionsAllowed(context, pcId, allowed)
        },
    )

    /** Reading a PC's threads and sessions after its catalog arrives, once at a time per PC. */
    private val agentRefreshes = java.util.concurrent.ConcurrentHashMap<String, Job>()
    private val agentSessionRefreshes = java.util.concurrent.ConcurrentHashMap<String, Job>()

    private fun onAgentEvent(pcId: String, event: Event) {
        var before = AgentsState()
        var after = AgentsState()
        var reread: String? = null
        _agents.update { all ->
            val current = all[pcId] ?: AgentsState()
            val result = AgentsModel.reduce(current, event)
            before = current
            after = result.state
            reread = result.reread
            if (result.state === current) all else all + (pcId to result.state)
        }
        reread?.let { agentReread(pcId, it) }
        when (event) {
            // Home shows what runs and waits without the Agents screen open: read it all once the PC says what it offers.
            is Event.AgentCatalog -> if (event.catalog != null) agentRefresh(pcId)
            // Allowed on the PC (it sends these only then): the whole list, not just this one.
            is Event.AgentSessionChanged -> if (before.sessionsAllowed != true) agentRefreshSessions(pcId)
            else -> Unit
        }
        val shown = agentForeground?.takeIf { it.first == pcId }?.second
        val shownSession = agentSessionForeground?.takeIf { it.first == pcId }?.second
        val state = after
        val notices = AgentNotifications.plan(
            pcId,
            event,
            state,
            shown,
            shownSession,
            kept = { session -> agentKeeps(pcId, session) },
        ) { thread -> agentFollows(pcId, thread, state) }
        if (notices.isNotEmpty()) AgentNotifications.apply(context, agentPcName(pcId), notices)
        // A session that ended or left the list isn't kept in the status bar any more (its notice went above).
        when (event) {
            is Event.AgentSessionChanged -> if (event.session.state == AgentSessionState.ENDED) agentForgetKept(pcId, event.session.id)
            is Event.AgentSessionRemoved -> agentForgetKept(pcId, event.session)
            else -> Unit
        }
    }

    private fun agentFollows(pcId: String, threadId: String, state: AgentsState): Boolean =
        state.thread(threadId)?.fromPhone == true || "$pcId/$threadId" in agentFollowed

    private fun agentKeeps(pcId: String, sessionId: String): Boolean = "$pcId/$sessionId" in _agentKeptSessions.value

    private fun agentForgetKept(pcId: String, sessionId: String) {
        _agentKeptSessions.update { it - "$pcId/$sessionId" }
    }

    private fun agentPcName(pcId: String): String =
        _state.value.nameOf(pcId)?.ifBlank { null } ?: context.getString(R.string.your_pc)

    /** The PC went away or was unpaired: what this phone knew of its threads and sessions goes with it. */
    private fun agentsGone(pcId: String) {
        _agents.update { it - pcId }
        agentWatchWanted.remove(pcId)
        agentSessionWatchWanted.remove(pcId)
        if (agentForeground?.first == pcId) agentForeground = null
        if (agentSessionForeground?.first == pcId) agentSessionForeground = null
        _agentKeptSessions.update { kept -> kept.filterNot { it.startsWith("$pcId/") }.toSet() }
        agentRefreshes.remove(pcId)?.cancel()
        agentSessionRefreshes.remove(pcId)?.cancel()
        AgentNotifications.cancelPc(context, pcId)
    }

    private fun updateAgents(pcId: String, change: (AgentsState) -> AgentsState) {
        _agents.update { all ->
            val current = all[pcId] ?: AgentsState()
            val next = change(current)
            if (next === current) all else all + (pcId to next)
        }
    }

    /** Something may be missing from the open transcript: read its newest page again, once at a time. */
    private fun agentReread(pcId: String, threadId: String) {
        val key = "$pcId/$threadId"
        if (agentRereads[key]?.isActive == true) return
        agentRereads[key] = scope.launch { agentRead(pcId, threadId, null) }
    }

    /** A PC's threads and then, when that can't make the PC prompt unasked, its sessions, after its catalog arrived. */
    private fun agentRefresh(pcId: String) {
        if (agentRefreshes[pcId]?.isActive == true) return
        agentRefreshes[pcId] = scope.launch {
            if (agentThreads(pcId) is AgentCall.Ok) agentSessionsIfAllowed(pcId)
        }
    }

    private fun agentRefreshSessions(pcId: String) {
        if (agentSessionRefreshes[pcId]?.isActive == true) return
        agentSessionRefreshes[pcId] = scope.launch { agentSessions(pcId) }
    }

    private suspend fun <T> agentCall(block: suspend (G7LinkNode) -> T): AgentCall<T> {
        startJob?.join()
        val current = node ?: return AgentCall.Failed(AgentFailure.OFFLINE)
        return try {
            AgentCall.Ok(block(current))
        } catch (e: G7LinkException) {
            AgentCall.Failed(agentFailure(e))
        }
    }

    private fun agentFailure(e: G7LinkException): AgentFailure = when (e) {
        is G7LinkException.Denied -> AgentFailure.DENIED
        is G7LinkException.Unsupported -> AgentFailure.UNSUPPORTED
        is G7LinkException.NotFound -> AgentFailure.NOT_FOUND
        is G7LinkException.Offline, is G7LinkException.NotPaired -> AgentFailure.OFFLINE
        is G7LinkException.Timeout -> AgentFailure.TIMEOUT
        else -> AgentFailure.OTHER
    }

    /** The PC's threads (and the catalog the core last heard, which may predate the screen). */
    suspend fun agentThreads(pcId: String): AgentCall<List<AgentThread>> {
        val result = agentCall { current ->
            val catalog = withContext(Dispatchers.IO) { runCatching { current.agentCatalog(pcId) }.getOrNull() }
            if (catalog != null) updateAgents(pcId) { if (it.catalog == catalog) it else it.copy(catalog = catalog) }
            current.agentThreads(pcId)
        }
        if (result is AgentCall.Ok) updateAgents(pcId) { AgentsModel.withThreads(it, result.value) }
        return result
    }

    /** Starts a thread on the PC; a phone only ever starts one that asks before each change. */
    suspend fun agentStart(pcId: String, agent: String, folder: String, prompt: String): AgentCall<AgentThread> {
        if (!AgentsModel.fits(prompt)) return AgentCall.Failed(AgentFailure.OTHER)
        val result = agentCall { it.agentStart(pcId, agent, folder, prompt, AgentMode.SUPERVISED) }
        if (result is AgentCall.Ok) {
            agentFollowed += "$pcId/${result.value.id}"
            updateAgents(pcId) { AgentsModel.withThread(it, result.value) }
        }
        return result
    }

    suspend fun agentSend(pcId: String, threadId: String, text: String): AgentCall<Unit> {
        if (!AgentsModel.fits(text)) return AgentCall.Failed(AgentFailure.OTHER)
        val result = agentCall { it.agentSend(pcId, threadId, text) }
        if (result is AgentCall.Ok) agentFollowed += "$pcId/$threadId"
        return result
    }

    suspend fun agentCancel(pcId: String, threadId: String): AgentCall<Unit> =
        agentCall { it.agentCancel(pcId, threadId) }

    /** The newest page of a thread, or the one before [before]; the open transcript takes it. */
    suspend fun agentRead(pcId: String, threadId: String, before: ULong?): AgentCall<AgentPage> {
        val result = agentCall { it.agentRead(pcId, threadId, before, AgentsModel.PAGE.toUInt()) }
        if (result is AgentCall.Ok) {
            updateAgents(pcId) {
                if (before == null) AgentsModel.withPage(it, result.value) else AgentsModel.withOlder(it, result.value)
            }
        }
        return result
    }

    /** Answers a request; the answer shows as given until the PC says who answered first. */
    suspend fun agentAnswer(pcId: String, token: String, allow: Boolean): AgentCall<Unit> {
        updateAgents(pcId) { AgentsModel.answering(it, token) }
        val result = agentCall { it.agentAnswer(pcId, token, allow) }
        if (result is AgentCall.Failed) updateAgents(pcId) { AgentsModel.answerFailed(it, token, result.failure) }
        return result
    }

    /** Asks the PC for the text of the thread this phone wants now ([agentWatchWanted]), in order. */
    private suspend fun agentSyncWatch(pcId: String): AgentCall<Unit> = agentWatchLock.withLock {
        val wanted = agentWatchWanted[pcId]
        agentCall { it.agentWatch(pcId, wanted) }
    }

    /**
     * A thread's screen came into view: its notification goes, the PC sends its
     * text, and its newest page is read.
     */
    suspend fun agentShown(pcId: String, threadId: String): AgentCall<AgentPage> {
        agentFollowed += "$pcId/$threadId"
        agentForeground = pcId to threadId
        updateAgents(pcId) { AgentsModel.opened(it, threadId) }
        AgentNotifications.cancel(context, pcId, threadId)
        agentWatchWanted[pcId] = threadId
        val watched = agentSyncWatch(pcId)
        if (watched is AgentCall.Failed) return AgentCall.Failed(watched.failure)
        return agentRead(pcId, threadId, null)
    }

    /** A thread's screen went out of view: its text stops, and what waits there notifies again. */
    fun agentHidden(pcId: String, threadId: String) {
        if (agentForeground == pcId to threadId) agentForeground = null
        agentWatchWanted.remove(pcId, threadId)
        scope.launch { agentSyncWatch(pcId) }
        agentRefreshNotice(pcId, threadId)
    }

    /** Left a thread's screen: its items are dropped. */
    fun agentClosed(pcId: String, threadId: String) = updateAgents(pcId) { AgentsModel.closed(it, threadId) }

    /** Shows what a thread's notification should show now. */
    private fun agentRefreshNotice(pcId: String, threadId: String) {
        val state = _agents.value[pcId] ?: return
        val shown = agentForeground?.takeIf { it.first == pcId }?.second
        val notice = AgentNotifications.threadNotice(pcId, state, threadId, shown) { agentFollows(pcId, it, state) }
        if (notice != null) AgentNotifications.apply(context, agentPcName(pcId), listOf(notice))
    }

    /** Allow or Deny on a request's notification. */
    fun agentAnswerFromNotification(pcId: String, threadId: String, token: String, allow: Boolean) {
        scope.launch {
            when (val result = agentAnswer(pcId, token, allow)) {
                // The PC says who answered (AgentAsked), and the notification follows.
                is AgentCall.Ok -> Unit
                is AgentCall.Failed ->
                    if (result.failure == AgentFailure.NOT_FOUND) {
                        agentRefreshNotice(pcId, threadId)
                    } else {
                        AgentNotifications.failed(context, pcId, threadId, R.string.agents_notify_answer_failed)
                    }
            }
        }
    }

    /** Stop on a working thread's notification. */
    fun agentStopFromNotification(pcId: String, threadId: String) {
        scope.launch {
            if (agentCancel(pcId, threadId) is AgentCall.Failed) {
                AgentNotifications.failed(context, pcId, threadId, R.string.agents_failed_generic)
            }
        }
    }

    /** Reply on a finished thread's notification. */
    fun agentReplyFromNotification(pcId: String, threadId: String, text: String) {
        scope.launch {
            when (agentSend(pcId, threadId, text.trim())) {
                is AgentCall.Ok -> AgentNotifications.cancel(context, pcId, threadId)
                is AgentCall.Failed -> AgentNotifications.failed(context, pcId, threadId, R.string.agents_notify_reply_failed)
            }
        }
    }

    // ---- Sessions started on a PC outside G7 (docs/protocol/agents.md §3) ----

    /**
     * The PC's sessions, unless asking could make the PC show its prompt
     * without the user having asked ([agentSessionsConsent]): then nothing is
     * sent and the list keeps its "Ask on <PC>" state.
     */
    suspend fun agentSessionsIfAllowed(pcId: String): AgentCall<List<AgentSession>>? =
        if (agentSessionsConsent.mayAskAutomatically(pcId)) agentSessions(pcId) else null

    /** "Ask on <PC>": the user asks, so the PC may now show its prompt; asked again for the rest of this run. */
    suspend fun agentSessionsAsk(pcId: String): AgentCall<List<AgentSession>> {
        agentSessionsConsent.userAsked(pcId)
        return agentSessions(pcId)
    }

    /**
     * The PC's sessions. Denied means its switch for this phone is off (the
     * first refusal makes the PC prompt): remembered until asked again.
     * Unsupported: it doesn't list them. Only for a request the user asked
     * for, or one [agentSessionsConsent] allows, or while the PC already sends
     * sessions (it's allowed).
     */
    private suspend fun agentSessions(pcId: String): AgentCall<List<AgentSession>> {
        val result = agentCall { it.agentSessions(pcId) }
        when (result) {
            is AgentCall.Ok -> {
                agentSessionsConsent.answered(pcId, app.g7link.android.ui.agents.SessionsConsent.Answer.LISTED)
                updateAgents(pcId) { AgentsModel.withSessions(it, result.value) }
            }
            is AgentCall.Failed -> agentSessionsRefused(pcId, result.failure)
        }
        return result
    }

    private fun agentSessionsRefused(pcId: String, failure: AgentFailure) {
        when (failure) {
            AgentFailure.DENIED -> {
                agentSessionsConsent.answered(pcId, app.g7link.android.ui.agents.SessionsConsent.Answer.REFUSED)
                updateAgents(pcId) { AgentsModel.sessionsOff(it) }
            }
            AgentFailure.UNSUPPORTED -> updateAgents(pcId) { AgentsModel.sessionsUnsupported(it) }
            AgentFailure.NOT_FOUND, AgentFailure.OFFLINE, AgentFailure.TIMEOUT, AgentFailure.OTHER -> Unit
        }
    }

    /** The newest page of a session, or the one before [before]; the open session takes it. */
    suspend fun agentSessionRead(pcId: String, sessionId: String, before: ULong?): AgentCall<AgentSessionPage> {
        val result = agentCall { it.agentSessionRead(pcId, sessionId, before, AgentsModel.PAGE.toUInt()) }
        when (result) {
            is AgentCall.Ok -> updateAgents(pcId) {
                if (before == null) AgentsModel.withSessionPage(it, result.value) else AgentsModel.withOlderSessionPage(it, result.value)
            }
            // Turned off on the PC while followed.
            is AgentCall.Failed -> if (result.failure == AgentFailure.DENIED) agentSessionsRefused(pcId, result.failure)
        }
        return result
    }

    /** Asks the PC for the items of the session this phone shows now ([agentSessionWatchWanted]), in order. */
    private suspend fun agentSessionSyncWatch(pcId: String): AgentCall<Unit> = agentSessionWatchLock.withLock {
        val wanted = agentSessionWatchWanted[pcId]
        agentCall { it.agentSessionWatch(pcId, wanted) }
    }

    /**
     * A followed session's view came into view (opened, back from the
     * background, or the PC reconnected): its notification waits, the PC sends
     * its items, and its newest page is read again. No gap is looked for.
     */
    suspend fun agentSessionShown(pcId: String, sessionId: String): AgentCall<AgentSessionPage> {
        agentSessionForeground = pcId to sessionId
        updateAgents(pcId) { AgentsModel.sessionOpened(it, sessionId) }
        AgentNotifications.cancelSession(context, pcId, sessionId)
        agentSessionWatchWanted[pcId] = sessionId
        val watched = agentSessionSyncWatch(pcId)
        if (watched is AgentCall.Failed) return AgentCall.Failed(watched.failure)
        return agentSessionRead(pcId, sessionId, null)
    }

    /** A followed session's view went out of view: its items stop, and a kept one shows in the status bar again. */
    fun agentSessionHidden(pcId: String, sessionId: String) {
        if (agentSessionForeground == pcId to sessionId) agentSessionForeground = null
        agentSessionWatchWanted.remove(pcId, sessionId)
        scope.launch { agentSessionSyncWatch(pcId) }
        agentRefreshSessionNotice(pcId, sessionId)
    }

    /** Left a followed session's view: its items are dropped. */
    fun agentSessionClosed(pcId: String, sessionId: String) = updateAgents(pcId) { AgentsModel.sessionClosed(it, sessionId) }

    /**
     * Keep in status bar, from the followed view (on) or Stop following on its
     * Live Update (off).
     */
    fun agentSessionKeep(pcId: String, sessionId: String, on: Boolean) {
        val key = "$pcId/$sessionId"
        _agentKeptSessions.update { if (on) it + key else it - key }
        if (on) {
            agentRefreshSessionNotice(pcId, sessionId)
        } else {
            AgentNotifications.cancelSession(context, pcId, sessionId)
        }
    }

    private fun agentRefreshSessionNotice(pcId: String, sessionId: String) {
        val state = _agents.value[pcId] ?: return
        val session = state.session(sessionId) ?: return
        val shownSession = agentSessionForeground?.takeIf { it.first == pcId }?.second
        val notice = AgentNotifications.sessionNotice(pcId, state, session, shownSession) { agentKeeps(pcId, it) }
        if (notice != null) AgentNotifications.apply(context, agentPcName(pcId), listOf(notice))
    }

    /**
     * Continue here: the PC makes the session a supervised thread started by
     * this phone, idle until something is sent. The session leaves the list.
     */
    suspend fun agentSessionContinue(pcId: String, sessionId: String): AgentCall<AgentThread> {
        val result = agentCall { it.agentSessionContinue(pcId, sessionId) }
        if (result is AgentCall.Ok) {
            agentFollowed += "$pcId/${result.value.id}"
            agentSessionKeep(pcId, sessionId, false)
            updateAgents(pcId) { state ->
                AgentsModel.withThread(state, result.value).let { it.copy(sessions = it.sessions.filterNot { s -> s.id == sessionId }) }
            }
        }
        return result
    }

    /**
     * A thread ([threadId]) or a followed session ([sessionId]) as Markdown,
     * to share: exactly one of the two. Kept in memory only; the caller hands
     * it to the share sheet.
     */
    suspend fun agentExport(pcId: String, threadId: String?, sessionId: String?): AgentCall<AgentExport> =
        agentCall { it.agentExport(pcId, threadId, sessionId) }

    /** A short message at the bottom of the app's screen. */
    fun say(message: String) {
        _messages.tryEmit(message)
    }

    // ---- Deck (phone -> PC) ----

    enum class DeckPressResult { Ok, Denied, Offline, NotFound, Failed }

    /** Presses a Deck tile on a paired PC (`deck.press`); says when it couldn't. */
    suspend fun deckPress(id: String, tile: String): DeckPressResult {
        startJob?.join()
        val node = node ?: run {
            _messages.tryEmit(context.getString(R.string.error_not_connected))
            return DeckPressResult.Offline
        }
        return try {
            node.deckPress(id, tile)
            DeckPressResult.Ok
        } catch (e: G7LinkException) {
            when (e) {
                is G7LinkException.Denied -> DeckPressResult.Denied
                is G7LinkException.Offline -> {
                    _messages.tryEmit(describe(e))
                    DeckPressResult.Offline
                }
                is G7LinkException.NotFound -> DeckPressResult.NotFound
                else -> {
                    _messages.tryEmit(describe(e))
                    DeckPressResult.Failed
                }
            }
        }
    }

    /** Sets the PC's master volume (`0..=100` or `200`/`201` for phone speaker relay) and/or mute state (`pc.audio.set`). */
    fun setPcAudio(id: String, volume: Int? = null, muted: Boolean? = null) {
        val isSpeakerRelayToggle = volume == 200 || volume == 201
        val wireVol = if (isSpeakerRelayToggle) {
            volume.toUByte()
        } else {
            volume?.coerceIn(0, 100)?.toUByte()
        }
        // Optimistic local state update so the slider, mute button, and phone speaker switch don't snap back while the RPC runs.
        _state.update { s ->
            val current = s.device(id)?.deckState
            if (current != null) {
                val updatedOutputs = when (volume) {
                    201 -> if (current.outputDevices.any { it.id == PHONE_SPEAKER_OUTPUT }) {
                        current.outputDevices
                    } else {
                        current.outputDevices + app.g7link.core.AudioOutputDevice(
                            id = PHONE_SPEAKER_OUTPUT,
                            name = "Phone speaker (G7 Link)",
                            isDefault = false,
                        )
                    }
                    200 -> current.outputDevices.filterNot { it.id == PHONE_SPEAKER_OUTPUT }
                    else -> current.outputDevices
                }
                s.reduce(
                    Event.DeckState(
                        id = id,
                        state = current.copy(
                            volume = if (isSpeakerRelayToggle) current.volume else (wireVol ?: current.volume),
                            muted = muted ?: current.muted,
                            outputDevices = updatedOutputs,
                        ),
                    ),
                )
            } else {
                s
            }
        }
        audioSender.offer(
            id,
            PcAudioRequest(
                relay = wireVol.takeIf { isSpeakerRelayToggle },
                volume = wireVol.takeIf { !isSpeakerRelayToggle },
                muted = muted,
            ),
        )
    }

    /** What a PC's audio should become; newer fields replace older ones. */
    private data class PcAudioRequest(val relay: UByte?, val volume: UByte?, val muted: Boolean?) {
        fun then(newer: PcAudioRequest) = PcAudioRequest(
            relay = newer.relay ?: relay,
            volume = newer.volume ?: volume,
            muted = newer.muted ?: muted,
        )
    }

    /** Sends each PC only its latest audio settings: a dragged slider never queues up requests. */
    private val audioSender = LatestValueSender<String, PcAudioRequest>(
        scope = scope,
        merge = PcAudioRequest::then,
        onError = { Log.w(TAG, "PC audio not set", it) },
    ) send@{ id, request ->
        val node = node ?: return@send
        if (request.relay != null) sendPcAudio(node, id, request.relay, null)
        if (request.volume != null || request.muted != null) sendPcAudio(node, id, request.volume, request.muted)
    }

    private suspend fun sendPcAudio(node: G7LinkNode, id: String, volume: UByte?, muted: Boolean?) {
        try {
            node.setPcAudio(id, volume, muted)
        } catch (e: G7LinkException) {
            if (e is G7LinkException.Denied) {
                val name = _state.value.nameOf(id).orEmpty()
                _messages.tryEmit(context.getString(R.string.remote_denied_title, name))
            } else if (e !is G7LinkException.Offline) {
                _messages.tryEmit(describe(e))
            }
        }
    }

    /** Makes [output], one of the PC's audio outputs, its default (`pc.audio.set { output }`). */
    fun setPcAudioOutput(id: String, output: String) {
        // Shown at once; the PC's next state confirms it or puts it back.
        _state.update { s ->
            val current = s.device(id)?.deckState ?: return@update s
            s.reduce(
                Event.DeckState(
                    id = id,
                    state = current.copy(
                        outputDevices = current.outputDevices.map {
                            if (it.id == PHONE_SPEAKER_OUTPUT) it else it.copy(isDefault = it.id == output)
                        },
                    ),
                ),
            )
        }
        enqueueRemoteOp { node ->
            try {
                node.setPcAudioOutput(id, output)
            } catch (e: G7LinkException) {
                if (e is G7LinkException.Denied) {
                    val name = _state.value.nameOf(id).orEmpty()
                    _messages.tryEmit(context.getString(R.string.remote_denied_title, name))
                } else if (e !is G7LinkException.Offline) {
                    _messages.tryEmit(describe(e))
                }
            }
        }
    }

    // ---- Phone storage (file manager on PC) ----

    /** Turns phone storage access in the PC's file manager on or off for `pcId` (off by default). */
    /** Stops reconnecting to a paired PC by itself ("Stop reconnecting"), or starts again. */
    fun setReconnect(pcId: String, on: Boolean) {
        _state.update { it.reduce(Event.ReconnectChanged(pcId, on)) }
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            runCatching { currentNode.setReconnect(pcId, on) }
                .onFailure { Log.w(TAG, "set reconnect failed", it) }
        }
    }

    fun setStorageEnabled(pcId: String, enabled: Boolean) {
        _state.update { it.withStorageEnabled(pcId, enabled) }
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node ?: return@launch
            runCatching { currentNode.setDeviceToggle(pcId, PhoneStorage.TOGGLE_NAME, enabled) }
            enqueueNotificationOp {
                it.updatePower(powerLevel(), capabilities(_state.value.notificationAccess))
            }
        }
    }

    fun dismissStorageRequest() {
        _state.update { it.copy(storageRequestedFrom = null) }
    }

    fun addSafFolder(treeUri: Uri) {
        PhoneStorage.addSafFolder(context, treeUri)
        refreshNotificationAccess()
    }

    fun removeSafFolder(name: String) {
        PhoneStorage.removeSafFolder(context, name)
        refreshNotificationAccess()
    }

    /**
     * Play on phone: up to [length] bytes from [offset] of the video file a PC offered.
     * Blocks; call off the main thread.
     */
    fun readWatchFile(pcId: String, offset: Long, length: Long): ByteArray {
        val running = node ?: throw java.io.IOException("G7 Link isn't running")
        return running.watchFileRead(pcId, offset.toULong(), length.toULong()).data
    }

    /** Lists files and folders in `path` on a paired PC (`storage.list`). */
    fun listPcFiles(
        deviceId: String,
        path: String,
        onResult: (PcFolder) -> Unit,
    ) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val folder = try {
                PcFolder(node?.listRemoteStorage(deviceId, path).orEmpty())
            } catch (e: G7LinkException) {
                Log.i(TAG, "can't list a PC's folder: $e")
                PcFolder(
                    emptyList(),
                    when (e) {
                        is G7LinkException.Denied -> R.string.pc_files_not_shared
                        is G7LinkException.Offline, is G7LinkException.NotPaired -> R.string.pc_files_offline
                        is G7LinkException.Unsupported -> R.string.pc_files_update
                        is G7LinkException.NotFound -> R.string.pc_files_not_found
                        else -> R.string.pc_files_error
                    },
                )
            }
            withContext(Dispatchers.Main) {
                onResult(folder)
            }
        }
    }

    /** Downloads `remotePath` from a paired PC (`storage.read`) and saves it into Downloads. */
    fun downloadPcFile(
        deviceId: String,
        remotePath: String,
        fileName: String,
        onSaved: (Uri?) -> Unit,
    ) {
        scope.launch(Dispatchers.IO) {
            startJob?.join()
            val currentNode = node
            if (currentNode == null) {
                withContext(Dispatchers.Main) { onSaved(null) }
                return@launch
            }
            val cacheDir = java.io.File(context.cacheDir, "pc_downloads").apply { mkdirs() }
            val safeName = fileName.replace(Regex("""[\\/:*?"<>|]"""), "_").ifBlank { "file" }
            val tempFile = java.io.File(cacheDir, "${System.currentTimeMillis()}_$safeName")
            val ok = runCatching {
                currentNode.downloadRemoteStorage(deviceId, remotePath, tempFile.absolutePath)
            }.getOrDefault(false)
            val savedUri = if (ok && tempFile.exists()) {
                ReceivedFiles.saveToDownloads(context, tempFile, safeName)
            } else {
                null
            }
            runCatching { tempFile.delete() }
            withContext(Dispatchers.Main) {
                onSaved(savedUri)
            }
        }
    }

    /** Asks an online relay peer to broadcast a Wake-on-LAN magic packet (`wake.relay`). */
    fun wakeViaRelay(relayPeerId: String, mac: String, broadcast: String? = null) {
        command { it.wakeViaRelay(relayPeerId, mac, broadcast) }
    }

    /** Reconnects to PCs that aren't connected and syncs connected ones. */
    fun refresh() {
        refreshNotificationAccess()
        command { it.refresh() }
    }

    fun unpair(id: String) = command { it.unpair(id) }

    /** Stops this phone ringing. */
    fun stopRinging() = command { it.stopRinging() }

    /** Records whether `pcId` is currently typing on this phone without mirroring. */
    fun setKeyboardFromPc(pcId: String, on: Boolean) {
        _state.update {
            it.copy(keyboardFromPc = if (on) it.keyboardFromPc + pcId else it.keyboardFromPc - pcId)
        }
    }

    /** Stops remote keyboard typing from `pcId` and notifies the PC. */
    fun stopKeyboardFromPc(pcId: String) {
        _state.update { it.copy(keyboardFromPc = it.keyboardFromPc - pcId) }
        val currentNode = node ?: return
        scope.launch(Dispatchers.IO) {
            runCatching { currentNode.mirrorStop(pcId, 0u) }
        }
    }

    /** Runs a command on the node, turning errors into a message. */
    private fun command(block: suspend (G7LinkNode) -> Unit) {
        scope.launch {
            startJob?.join()
            val node = node ?: return@launch
            try {
                block(node)
            } catch (e: G7LinkException) {
                _messages.tryEmit(describe(e))
            }
        }
    }

    private fun describe(e: G7LinkException): String = when (e) {
        is G7LinkException.Offline -> context.getString(R.string.error_not_connected)
        is G7LinkException.NotPaired -> context.getString(R.string.error_not_paired)
        is G7LinkException.Timeout -> context.getString(R.string.error_timed_out)
        is G7LinkException.Unsupported -> context.getString(R.string.error_unsupported)
        is G7LinkException.QueueFull -> e.reason.ifBlank { context.getString(R.string.send_queue_full) }
        else -> context.getString(R.string.error_generic)
    }

    enum class RemoteAccess { Allowed, Denied, Offline, Unsupported }

    private companion object {
        const val TAG = "G7Link"
        /** The protocol's bounds on one pointer move and one scroll (`remote::MAX_MOVE_DELTA`, `MAX_SCROLL_DELTA`). */
        const val MAX_MOVE_DELTA = 4000f
        const val MAX_SCROLL_DELTA = 200f
        /** The largest image a PC takes on its clipboard (`clip::MAX_IMAGE_BYTES`). */
        const val MAX_CLIPBOARD_PNG_BYTES = 32 * 1024 * 1024
        /** How long a Bluetooth connect waits for the core to start. */
        const val BLE_START_WAIT_MS = 10_000L
        /** A PC asking its user is asked again this often, for this long (§2.3). */
        const val BLE_ASK_POLL_MS = 1_500L
        const val BLE_ASK_WAIT_MS = 60_000L
        /** How long the PC's window is looked for once it's open; with no network, its own window lasts 45 s. */
        const val BLE_CONNECT_WAIT_MS = 25_000L
        const val BLE_NO_NETWORK_SCAN_MS = 45_000L
        const val BLE_LINK_ATTEMPTS = 3
        /** Time for the PC to hear this phone's new capabilities before it's asked again. */
        const val BLE_CAPABILITIES_SETTLE_MS = 1_000L
        const val BLE_LINK_POLL_MS = 250L
        /** The touchpad's hold: asked again this often while it shows, and kept this long after it's left (§6.3). */
        const val BLE_HOLD_RENEW_MS = 20_000L
        const val BLE_HOLD_LINGER_MS = 30_000L
        const val BLE_HOLD_TICK_MS = 1_000L
        /** A renewal that finds the window open and no link looks for it this long. */
        const val BLE_HOLD_LINK_MS = 12_000L
        /** An attempt that found the PC is waited for this long past the look, at most: its own limit is 20 s. */
        const val BLE_LINK_FINISH_MS = 21_000L
        /** Offered while notification access is granted (docs/protocol/capabilities.md). */
        val NOTIFICATION_CAPABILITIES = listOf("notify.mirror", "notify.reply", "notify.live")
        /** Sharing this phone's players, which also needs notification access. */
        val MEDIA_CAPABILITIES = listOf("media.control")
        /** Accepting the PC's clipboard, and sending this one when asked. */
        val CLIPBOARD_CAPABILITIES = listOf("clip.write", "clip.share", "mirror.capture", "mirror.view")
        /** With access to the phone's photos. */
        val PHOTO_CAPABILITIES = listOf("photos.read")
    }
}

/** A folder on a PC: its files, or why they can't be listed (a string resource). */
data class PcFolder(
    val entries: List<RemoteStorageEntry>,
    @androidx.annotation.StringRes val problem: Int? = null,
)

/** Keys that leave the PC's caret where it is: what the typing box holds still matches. */
private val KEYS_KEEPING_CARET =
    setOf("volume_up", "volume_down", "volume_mute", "play_pause", "next_track", "prev_track")

/**
 * Icons of the apps focused on the PCs, for the typing box. A PC names each
 * by a key (16 hex characters of its PNG's hash), so a key always means the
 * same icon. The bytes are the PC's: bounded and checked before decoding.
 */
internal object PcAppIcons {
    /** In the app's cache folder: a folder per PC, a PNG per key. */
    const val DIR = "pc-icons"
    const val IN_MEMORY = 32
    const val FILES_PER_PC = 64
    const val MAX_BYTES = 16 * 1024
    const val MAX_PX = 256

    /** A key as the PC sends it: 16 lowercase hex characters, safe as a file name. */
    fun isKey(key: String): Boolean = key.length == 16 && key.all { it in '0'..'9' || it in 'a'..'f' }

    /** A PC id that's safe as a folder name. */
    fun isPcFolder(pcId: String): Boolean = pcId.length in 1..128 && pcId.all { it in 'a'..'z' || it in '0'..'9' }

    /** Whether a picture of [size] bytes, [width] by [height] pixels, is one to decode. */
    fun fits(size: Int, width: Int, height: Int): Boolean =
        size in 1..MAX_BYTES && width in 1..MAX_PX && height in 1..MAX_PX

    /** Decodes a PNG once its size and dimensions check out: a small file can claim a huge image. */
    fun decode(bytes: ByteArray): Bitmap? {
        if (bytes.size !in 1..MAX_BYTES) return null
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outMimeType != "image/png" || !fits(bytes.size, bounds.outWidth, bounds.outHeight)) return null
        return runCatching { BitmapFactory.decodeByteArray(bytes, 0, bytes.size) }.getOrNull()
    }

    /** The icon kept in [file], marked as just used; one that won't decode goes. */
    fun read(file: java.io.File): Bitmap? {
        if (!file.isFile) return null
        val icon = runCatching { if (file.length() <= MAX_BYTES) decode(file.readBytes()) else null }.getOrNull()
        if (icon == null) {
            runCatching { file.delete() }
            return null
        }
        runCatching { file.setLastModified(System.currentTimeMillis()) }
        return icon
    }

    /** Keeps [bytes] in [file], then only the [FILES_PER_PC] icons of that PC used last. */
    fun save(file: java.io.File, bytes: ByteArray) {
        val dir = file.parentFile ?: return
        runCatching {
            dir.mkdirs()
            val temp = java.io.File.createTempFile("icon", ".tmp", dir)
            try {
                temp.writeBytes(bytes)
                temp.renameTo(file)
            } finally {
                temp.delete()
            }
            dir.listFiles()
                ?.filter { it.isFile && it.name.endsWith(".png") }
                ?.sortedByDescending { it.lastModified() }
                ?.drop(FILES_PER_PC)
                ?.forEach { it.delete() }
        }
    }
}
