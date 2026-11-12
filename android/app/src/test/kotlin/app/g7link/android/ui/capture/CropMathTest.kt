// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CropMathTest {
    private val eps = 1e-5f

    private fun assertRect(expected: CropRect, actual: CropRect) {
        assertEquals(expected.left, actual.left, eps)
        assertEquals(expected.top, actual.top, eps)
        assertEquals(expected.right, actual.right, eps)
        assertEquals(expected.bottom, actual.bottom, eps)
    }

    @Test
    fun `a touch grabs the corner, the edge or the inside it lands on`() {
        val rect = CropRect(0.2f, 0.2f, 0.8f, 0.8f)
        val slop = 0.05f
        assertEquals(CropHandle.TopLeft, CropMath.handleAt(rect, 0.21f, 0.18f, slop, slop))
        assertEquals(CropHandle.BottomRight, CropMath.handleAt(rect, 0.82f, 0.79f, slop, slop))
        assertEquals(CropHandle.Top, CropMath.handleAt(rect, 0.5f, 0.22f, slop, slop))
        assertEquals(CropHandle.Left, CropMath.handleAt(rect, 0.19f, 0.5f, slop, slop))
        assertEquals(CropHandle.Move, CropMath.handleAt(rect, 0.5f, 0.5f, slop, slop))
        assertNull(CropMath.handleAt(rect, 0.05f, 0.5f, slop, slop))
        assertNull(CropMath.handleAt(rect, 0.5f, 0.95f, slop, slop))
    }

    @Test
    fun `dragging an edge or a corner moves only its sides`() {
        val rect = CropRect(0.2f, 0.2f, 0.8f, 0.8f)
        assertRect(CropRect(0.3f, 0.2f, 0.8f, 0.8f), CropMath.drag(rect, CropHandle.Left, 0.1f, 0.3f, 0.1f, 0.1f))
        assertRect(CropRect(0.2f, 0.1f, 0.7f, 0.8f), CropMath.drag(rect, CropHandle.TopRight, -0.1f, -0.1f, 0.1f, 0.1f))
        assertRect(CropRect(0.2f, 0.2f, 0.8f, 0.9f), CropMath.drag(rect, CropHandle.Bottom, 0.4f, 0.1f, 0.1f, 0.1f))
    }

    @Test
    fun `the crop stays inside the picture and keeps its smallest size`() {
        val rect = CropRect(0.2f, 0.2f, 0.8f, 0.8f)
        // Past the picture's edge: up to it.
        assertRect(CropRect(0f, 0f, 0.8f, 0.8f), CropMath.drag(rect, CropHandle.TopLeft, -0.5f, -0.5f, 0.1f, 0.1f))
        // Past the other side: no smaller than the minimum.
        assertRect(CropRect(0.7f, 0.2f, 0.8f, 0.8f), CropMath.drag(rect, CropHandle.Left, 0.9f, 0f, 0.1f, 0.1f))
        // Moving keeps its size and stops at the edges.
        assertRect(CropRect(0.4f, 0f, 1f, 0.6f), CropMath.drag(rect, CropHandle.Move, 0.5f, -0.5f, 0.1f, 0.1f))
        // A minimum bigger than the crop never makes it grow, or throw.
        val small = CropRect(0.5f, 0.5f, 0.55f, 0.55f)
        assertRect(small, CropMath.drag(small, CropHandle.Right, -0.2f, 0f, 0.3f, 0.3f))
        // The whole picture, dragged outward, stays the whole picture.
        assertRect(CropRect.FULL, CropMath.drag(CropRect.FULL, CropHandle.BottomRight, 0.3f, 0.3f, 0.1f, 0.1f))
    }

    @Test
    fun `the crop in pixels is inside the picture and at least a pixel`() {
        assertEquals(PixelRect(0, 0, 1920, 1080), CropMath.pixels(CropRect.FULL, 1920, 1080))
        assertEquals(PixelRect(480, 270, 1440, 810), CropMath.pixels(CropRect(0.25f, 0.25f, 0.75f, 0.75f), 1920, 1080))
        val sliver = CropMath.pixels(CropRect(0.5f, 0.5f, 0.5f, 0.5f), 100, 100)
        assertEquals(1, sliver.width)
        assertEquals(1, sliver.height)
        val edge = CropMath.pixels(CropRect(1f, 1f, 1f, 1f), 100, 100)
        assertEquals(PixelRect(99, 99, 100, 100), edge)
    }

    @Test
    fun `a picture fits its box whole and centred`() {
        val px = 0.01f
        val wide = CropMath.fit(1000f, 1000f, 1920f, 1080f)
        assertEquals(0f, wide.left, px)
        assertEquals(1000f, wide.width, px)
        assertEquals(562.5f, wide.height, px)
        assertEquals((1000f - 562.5f) / 2f, wide.top, px)
        val tall = CropMath.fit(1000f, 500f, 100f, 100f)
        assertEquals(250f, tall.left, px)
        assertEquals(500f, tall.width, px)
        assertEquals(FitRect(0f, 0f, 0f, 0f), CropMath.fit(0f, 0f, 10f, 10f))
    }
}
