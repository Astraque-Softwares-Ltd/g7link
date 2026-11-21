// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.widget

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.view.View
import android.widget.RemoteViews
import app.g7link.android.R
import app.g7link.android.agents.AgentsSurface
import app.g7link.android.agents.AgentsSurfaceStore
import app.g7link.android.agents.AgentsSurfaces
import app.g7link.android.agents.SurfaceRow
import app.g7link.android.agents.SurfaceState
import app.g7link.android.ui.MainActivity

/**
 * The Agents home-screen widget (contract §V8.3, boards 07c, 07d): "Agents ·
 * Kilele", then up to three threads, each with its state dot, title and
 * "Claude · duka-pos · Needs you". Home screen only (`widgetCategory`), never
 * a request's text, no Allow or Deny: a row opens its thread, the header the
 * list. With nothing to show, "No threads are running."; while the PC can't
 * be reached, its last rows dimmed and "Kilele isn't reachable.". Drawn from
 * the core's snapshot when it's written, never on a timer.
 */
class AgentsWidgetProvider : AppWidgetProvider() {
    override fun onUpdate(context: Context, appWidgetManager: AppWidgetManager, appWidgetIds: IntArray) {
        draw(context, appWidgetManager, appWidgetIds, AgentsSurfaceStore.read(context))
    }

    companion object {
        private val ROWS = listOf(
            Row(R.id.agents_widget_row1, R.id.agents_widget_dot1, R.id.agents_widget_title1, R.id.agents_widget_sub1),
            Row(R.id.agents_widget_row2, R.id.agents_widget_dot2, R.id.agents_widget_title2, R.id.agents_widget_sub2),
            Row(R.id.agents_widget_row3, R.id.agents_widget_dot3, R.id.agents_widget_title3, R.id.agents_widget_sub3),
        )

        private val ACCENT = Color.parseColor("#B292D0")
        private val TEXT = Color.parseColor("#F2F2F3")
        private val MUTED = Color.parseColor("#A1A1A8")
        private val DIM_TITLE = Color.parseColor("#99F2F2F3")
        private val DIM_SUB = Color.parseColor("#99A1A1A8")

        /** The dark palette's state colours (AgentPalette), as the app's thread list shows them. */
        private fun dotColour(state: SurfaceState): Int = when (state) {
            SurfaceState.NEEDS_YOU -> Color.parseColor("#EF9F27")
            SurfaceState.WORKING -> Color.parseColor("#7CB7EE")
            SurfaceState.DONE, SurfaceState.PLAN -> Color.parseColor("#97C459")
            SurfaceState.FAILED -> Color.parseColor("#E5736F")
            SurfaceState.PAUSED, SurfaceState.STOPPED -> Color.parseColor("#6F6F78")
        }

        private fun stateWord(context: Context, state: SurfaceState): String = context.getString(
            when (state) {
                SurfaceState.NEEDS_YOU -> R.string.agents_state_waiting
                SurfaceState.WORKING -> R.string.agents_state_working
                SurfaceState.DONE -> R.string.agents_state_done
                SurfaceState.PLAN -> R.string.agents_state_plan_ready
                SurfaceState.FAILED -> R.string.agents_state_failed
                SurfaceState.PAUSED -> R.string.agents_state_paused
                SurfaceState.STOPPED -> R.string.agents_state_interrupted
            },
        )

        /** Draws every Agents widget again from [surface]. */
        fun refresh(context: Context, surface: AgentsSurface = AgentsSurfaceStore.read(context)) {
            val manager = AppWidgetManager.getInstance(context) ?: return
            val ids = runCatching { manager.getAppWidgetIds(ComponentName(context, AgentsWidgetProvider::class.java)) }.getOrNull() ?: return
            if (ids.isEmpty()) return
            draw(context, manager, ids, surface)
        }

        private fun draw(context: Context, manager: AppWidgetManager, ids: IntArray, surface: AgentsSurface) {
            val view = AgentsSurfaces.widget(surface)
            val views = RemoteViews(context.packageName, R.layout.widget_agents)
            val heading = view.pcName?.takeIf { it.isNotBlank() }?.let { context.getString(R.string.agents_widget_header, it) }
                ?: context.getString(R.string.agents_title)
            views.setTextViewText(R.id.agents_widget_heading, heading)
            // The G7 mark in the accent, as the boards draw it (set here: a widget's layout can't take app:tint).
            views.setInt(R.id.agents_widget_mark, "setColorFilter", ACCENT)
            views.setOnClickPendingIntent(R.id.agents_widget_header, open(context, view.pcId, null))
            for ((index, slot) in ROWS.withIndex()) {
                val row = view.rows.getOrNull(index)
                if (row == null) {
                    views.setViewVisibility(slot.root, View.GONE)
                    continue
                }
                fill(context, views, slot, row, view.pcId, dim = view.offline)
            }
            views.setViewVisibility(R.id.agents_widget_empty, if (view.rows.isEmpty() && !view.offline) View.VISIBLE else View.GONE)
            if (view.offline && view.pcName != null) {
                views.setTextViewText(R.id.agents_widget_note, context.getString(R.string.agents_unreachable_widget, view.pcName))
                views.setViewVisibility(R.id.agents_widget_note, View.VISIBLE)
            } else {
                views.setViewVisibility(R.id.agents_widget_note, View.GONE)
            }
            runCatching { manager.updateAppWidget(ids, views) }
        }

        private fun fill(context: Context, views: RemoteViews, slot: Row, row: SurfaceRow, pcId: String?, dim: Boolean) {
            views.setViewVisibility(slot.root, View.VISIBLE)
            views.setInt(slot.dot, "setColorFilter", dotColour(row.state))
            views.setInt(slot.dot, "setImageAlpha", if (dim) DIM_ALPHA else FULL_ALPHA)
            views.setTextViewText(slot.title, row.title)
            views.setTextColor(slot.title, if (dim) DIM_TITLE else TEXT)
            val sub = listOf(row.agent, row.folder, stateWord(context, row.state)).filter { it.isNotBlank() }.joinToString(" · ")
            views.setTextViewText(slot.sub, sub)
            views.setTextColor(slot.sub, if (dim) DIM_SUB else MUTED)
            if (pcId != null) views.setOnClickPendingIntent(slot.root, open(context, pcId, row.threadId))
        }

        /** Opens the app at [pcId]'s agents, and [threadId] when given; each its own intent (the data differs). */
        private fun open(context: Context, pcId: String?, threadId: String?): PendingIntent {
            val intent = Intent(context, MainActivity::class.java)
                .setData(Uri.Builder().scheme("g7link-agents-widget").authority(pcId ?: "none").appendPath(threadId ?: "list").build())
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (pcId != null) intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, pcId)
            if (pcId != null && threadId != null) intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_THREAD, threadId)
            val request = "agents-widget:${pcId.orEmpty()}:${threadId.orEmpty()}".hashCode()
            return PendingIntent.getActivity(context, request, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }

        private const val FULL_ALPHA = 255
        private const val DIM_ALPHA = 153
    }

    private data class Row(val root: Int, val dot: Int, val title: Int, val sub: Int)
}
