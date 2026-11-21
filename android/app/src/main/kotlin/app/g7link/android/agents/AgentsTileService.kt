// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.agents

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import app.g7link.android.R
import app.g7link.android.ui.MainActivity

/**
 * The Agents Quick Settings tile (contract §V8.3, board 07a, 07b): counts
 * only, on any screen ("1 needs you", lit; "2 working"; "Nothing running";
 * "Kilele isn't reachable"), from the snapshot the core writes. A tap opens
 * the thread that has waited longest, else the list, unlocking first; it
 * never answers or starts anything.
 */
class AgentsTileService : TileService() {
    override fun onTileAdded() {
        super.onTileAdded()
        draw()
    }

    override fun onStartListening() {
        super.onStartListening()
        draw()
    }

    private fun draw() {
        val tile = qsTile ?: return
        val view = AgentsSurfaces.tile(AgentsSurfaceStore.read(this))
        val line = lineText(this, view.line)
        tile.label = getString(R.string.agents_title)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.subtitle = line
        } else {
            tile.contentDescription = "${getString(R.string.agents_title)}, $line"
        }
        tile.state = if (view.active) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        tile.icon = Icon.createWithResource(this, R.drawable.ic_notification)
        runCatching { tile.updateTile() }
    }

    override fun onClick() {
        super.onClick()
        val view = AgentsSurfaces.tile(AgentsSurfaceStore.read(this))
        val open = Runnable { open(view.pcId, view.threadId) }
        if (isLocked) unlockAndRun(open) else open.run()
    }

    @SuppressLint("StartActivityAndCollapseDeprecated") // The Intent overload is the only one before Android 14.
    private fun open(pcId: String?, threadId: String?) {
        val intent = Intent(this, MainActivity::class.java)
            .setData(Uri.Builder().scheme("g7link-agents-tile").authority(pcId ?: "none").appendPath(threadId ?: "list").build())
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (pcId != null) intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, pcId)
        if (pcId != null && threadId != null) intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_THREAD, threadId)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startActivityAndCollapse(
                PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT),
            )
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }

    companion object {
        /** The tile's second line. */
        fun lineText(context: Context, line: TileLine): String = when (line) {
            is TileLine.NeedsYou -> context.resources.getQuantityString(R.plurals.agents_tile_needs_you, line.count, line.count)
            is TileLine.Working -> context.resources.getQuantityString(R.plurals.agents_tile_working, line.count, line.count)
            TileLine.Nothing -> context.getString(R.string.agents_tile_nothing)
            is TileLine.Unreachable -> context.getString(R.string.agents_tile_unreachable, line.pcName)
        }

        /** Asks Android to draw the tile again (when it's in Quick Settings). */
        fun refresh(context: Context) {
            runCatching { requestListeningState(context, ComponentName(context, AgentsTileService::class.java)) }
        }
    }
}
