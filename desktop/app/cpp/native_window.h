// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QtCore/QPointer>
#include <QtQml/qqmlregistration.h>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickWindow>

#include <vector>

// A frameless top-level window: the app draws its own title bar and caption
// buttons, and the window handles dragging, double-click to maximize and
// resizing from its edges.
class NativeWindow : public QQuickWindow
{
    Q_OBJECT
    QML_ELEMENT

    // Paints the window background dark, behind the content.
    Q_PROPERTY(bool darkFrame READ darkFrame WRITE setDarkFrame NOTIFY darkFrameChanged)
    // Height of the draggable title strip, in logical pixels.
    Q_PROPERTY(qreal captionHeight READ captionHeight WRITE setCaptionHeight NOTIFY captionHeightChanged)
    // The app's maximize button: clicks on it don't drag the window.
    Q_PROPERTY(QQuickItem *maximizeButton READ maximizeButton WRITE setMaximizeButton NOTIFY maximizeButtonChanged)

public:
    explicit NativeWindow(QWindow *parent = nullptr);

    bool darkFrame() const { return m_darkFrame; }
    void setDarkFrame(bool dark);
    qreal captionHeight() const { return m_captionHeight; }
    void setCaptionHeight(qreal height);
    QQuickItem *maximizeButton() const { return m_maximizeButton; }
    void setMaximizeButton(QQuickItem *item);

    // Interactive items inside the title strip (buttons, search fields):
    // clicks there go to the item instead of dragging the window.
    Q_INVOKABLE void addCaptionHole(QQuickItem *item);
    Q_INVOKABLE void removeCaptionHole(QQuickItem *item);

    Q_INVOKABLE void toggleMaximized();
    // Shows, restores and focuses the window, even when another app is in
    // front (the caller must have the right to take the foreground).
    Q_INVOKABLE void bringToFront();
    // Flashes the taskbar button until the window is activated.
    Q_INVOKABLE void flash();
    // Releases scene-graph and GPU resources and destroys the native window
    // when closing to the tray.
    Q_INVOKABLE void releaseAndTeardown();
    // Captures the rendered window contents to an image file on disk.
    Q_INVOKABLE bool grabToFile(const QString &path);

    bool isCaptionHoleAt(const QPointF &scenePoint) const;

signals:
    void darkFrameChanged();
    void captionHeightChanged();
    void maximizeButtonChanged();
    void windowVisibilityChanged();
    void closeRequested();

protected:
    bool event(QEvent *event) override;

private:
    void applyFrame();
    bool itemContains(const QQuickItem *item, const QPointF &scenePoint) const;

    bool m_darkFrame = false;
    qreal m_captionHeight = 48;
    QPointer<QQuickItem> m_maximizeButton;
    std::vector<QPointer<QQuickItem>> m_holes;
};
