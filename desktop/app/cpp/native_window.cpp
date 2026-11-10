// SPDX-License-Identifier: GPL-3.0-or-later
#include "native_window.h"
#include "app_helpers.h"

#include <QtGui/QCursor>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtGui/QMouseEvent>
#include <QtGui/QPlatformSurfaceEvent>
#include <QtQml/QtQml>

#include <algorithm>

#include <dlfcn.h>

namespace {

int ignoreX11Error(void *, void *)
{
    return 0;
}

void activateX11Window(WId winId)
{
    if (!winId || qEnvironmentVariableIsEmpty("DISPLAY"))
        return;
    // On Wayland, QWindow::winId() is not an X11 Window ID even when XWayland sets DISPLAY.
    if (QGuiApplication::platformName() != QLatin1String("xcb"))
        return;

    void *lib = dlopen("libX11.so.6", RTLD_LAZY | RTLD_LOCAL);
    if (!lib)
        return;

    using XErrorHandlerFn = int (*)(void *, void *);
    using XSetErrorHandlerFn = XErrorHandlerFn (*)(XErrorHandlerFn);
    using XOpenDisplayFn = void *(*)(const char *);
    using XCloseDisplayFn = int (*)(void *);
    using XDefaultRootWindowFn = unsigned long (*)(void *);
    using XInternAtomFn = unsigned long (*)(void *, const char *, int);
    using XSendEventFn = int (*)(void *, unsigned long, int, long, void *);
    using XMapRaisedFn = int (*)(void *, unsigned long);
    using XSetInputFocusFn = int (*)(void *, unsigned long, int, unsigned long);
    using XSyncFn = int (*)(void *, int);
    using XFlushFn = int (*)(void *);

    auto xSetErrorHandler = reinterpret_cast<XSetErrorHandlerFn>(dlsym(lib, "XSetErrorHandler"));
    auto xOpenDisplay = reinterpret_cast<XOpenDisplayFn>(dlsym(lib, "XOpenDisplay"));
    auto xCloseDisplay = reinterpret_cast<XCloseDisplayFn>(dlsym(lib, "XCloseDisplay"));
    auto xDefaultRootWindow = reinterpret_cast<XDefaultRootWindowFn>(dlsym(lib, "XDefaultRootWindow"));
    auto xInternAtom = reinterpret_cast<XInternAtomFn>(dlsym(lib, "XInternAtom"));
    auto xSendEvent = reinterpret_cast<XSendEventFn>(dlsym(lib, "XSendEvent"));
    auto xMapRaised = reinterpret_cast<XMapRaisedFn>(dlsym(lib, "XMapRaised"));
    auto xSetInputFocus = reinterpret_cast<XSetInputFocusFn>(dlsym(lib, "XSetInputFocus"));
    auto xSync = reinterpret_cast<XSyncFn>(dlsym(lib, "XSync"));
    auto xFlush = reinterpret_cast<XFlushFn>(dlsym(lib, "XFlush"));

    if (xOpenDisplay && xCloseDisplay && xDefaultRootWindow && xInternAtom && xSendEvent && xFlush) {
        if (void *dpy = xOpenDisplay(nullptr)) {
            XErrorHandlerFn prevHandler = xSetErrorHandler ? xSetErrorHandler(ignoreX11Error) : nullptr;
            const unsigned long root = xDefaultRootWindow(dpy);
            const unsigned long window = static_cast<unsigned long>(winId);
            const unsigned long netActiveWindow = xInternAtom(dpy, "_NET_ACTIVE_WINDOW", 0);

            if (xMapRaised)
                xMapRaised(dpy, window);

            struct XClientMessageEventCompat {
                int type;
                unsigned long serial;
                int send_event;
                void *display;
                unsigned long window;
                unsigned long message_type;
                int format;
                long l[5];
                long pad[12];
            } ev {};
            ev.type = 33; // ClientMessage
            ev.send_event = 1;
            ev.display = dpy;
            ev.window = window;
            ev.message_type = netActiveWindow;
            ev.format = 32;
            ev.l[0] = 2; // Source indication: 2 = pager / direct user action (bypasses WM focus-stealing prevention)
            ev.l[1] = 0; // CurrentTime
            ev.l[2] = 0;

            constexpr long kSubstructureRedirectMask = (1L << 20);
            constexpr long kSubstructureNotifyMask = (1L << 19);
            xSendEvent(dpy, root, 0, kSubstructureRedirectMask | kSubstructureNotifyMask, &ev);

            if (xSetInputFocus)
                xSetInputFocus(dpy, window, 1 /* RevertToPointerRoot */, 0 /* CurrentTime */);

            if (xSync)
                xSync(dpy, 0);
            else
                xFlush(dpy);
            if (xSetErrorHandler)
                xSetErrorHandler(prevHandler);
            xCloseDisplay(dpy);
        }
    }
    dlclose(lib);
}

class LinuxWindowEventFilter : public QObject
{
public:
    explicit LinuxWindowEventFilter(NativeWindow *window)
        : QObject(window)
        , window_(window)
    {
    }

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        if (watched != window_)
            return QObject::eventFilter(watched, event);

        switch (event->type()) {
        case QEvent::MouseMove: {
            auto *me = static_cast<QMouseEvent *>(event);
            if (me->buttons() == Qt::NoButton) {
                pendingMove_ = false;
                manualMove_ = false;
                const Qt::Edges edges = hitResizeEdges(me->position());
                updateCursorForEdges(edges);
            } else if (me->buttons() & Qt::LeftButton) {
                if (pendingMove_) {
                    if ((me->position() - pressPos_).manhattanLength() >= 4) {
                        pendingMove_ = false;
                        if (!window_->startSystemMove()) {
                            manualMove_ = true;
                        }
                        return true;
                    }
                } else if (manualMove_) {
                    const QPointF delta = me->globalPosition() - globalPressPos_;
                    window_->setPosition(winStartPos_ + delta.toPoint());
                    return true;
                }
            }
            break;
        }
        case QEvent::MouseButtonPress: {
            auto *me = static_cast<QMouseEvent *>(event);
            if (me->button() == Qt::LeftButton) {
                pendingMove_ = false;
                manualMove_ = false;
                const QPointF pos = me->position();
                const Qt::Edges edges = hitResizeEdges(pos);
                if (edges != Qt::Edges()) {
                    window_->startSystemResize(edges);
                    return true;
                }
                const qreal titleBarHeight = window_->captionHeight();
                if (pos.y() < titleBarHeight
                    && !window_->isCaptionHoleAt(pos)
                    && !hitInteractiveControl(window_->contentItem(), pos)) {
                    pendingMove_ = true;
                    pressPos_ = pos;
                    globalPressPos_ = me->globalPosition();
                    winStartPos_ = window_->position();
                }
            }
            break;
        }
        case QEvent::MouseButtonRelease: {
            auto *me = static_cast<QMouseEvent *>(event);
            if (me->button() == Qt::LeftButton) {
                pendingMove_ = false;
                if (manualMove_) {
                    manualMove_ = false;
                    return true;
                }
            }
            break;
        }
        case QEvent::MouseButtonDblClick: {
            auto *me = static_cast<QMouseEvent *>(event);
            if (me->button() == Qt::LeftButton) {
                pendingMove_ = false;
                manualMove_ = false;
                const QPointF pos = me->position();
                const Qt::Edges edges = hitResizeEdges(pos);
                const qreal titleBarHeight = window_->captionHeight();
                if (edges == Qt::Edges() && pos.y() < titleBarHeight
                    && !window_->isCaptionHoleAt(pos)
                    && !hitInteractiveControl(window_->contentItem(), pos)) {
                    window_->toggleMaximized();
                    return true;
                }
            }
            break;
        }
        case QEvent::Leave:
            if (borderCursorActive_) {
                window_->unsetCursor();
                borderCursorActive_ = false;
            }
            break;
        default:
            break;
        }
        return QObject::eventFilter(watched, event);
    }

private:
    Qt::Edges hitResizeEdges(const QPointF &pos) const
    {
        if (!window_ || window_->visibility() == QWindow::Maximized
            || window_->minimumSize() == window_->maximumSize()) {
            return {};
        }
        if (pos.y() < window_->captionHeight()
            && (window_->isCaptionHoleAt(pos) || hitInteractiveControl(window_->contentItem(), pos))) {
            return {};
        }
        const qreal w = window_->width();
        const qreal h = window_->height();
        const qreal b = resizeBorderWidth_;
        Qt::Edges edges;
        if (pos.x() < b)
            edges |= Qt::LeftEdge;
        else if (pos.x() >= w - b)
            edges |= Qt::RightEdge;
        if (pos.y() < b)
            edges |= Qt::TopEdge;
        else if (pos.y() >= h - b)
            edges |= Qt::BottomEdge;
        return edges;
    }

    void updateCursorForEdges(Qt::Edges edges)
    {
        if (edges == Qt::Edges()) {
            if (borderCursorActive_) {
                window_->unsetCursor();
                borderCursorActive_ = false;
            }
            return;
        }
        Qt::CursorShape shape = Qt::ArrowCursor;
        if ((edges == (Qt::TopEdge | Qt::LeftEdge)) || (edges == (Qt::BottomEdge | Qt::RightEdge)))
            shape = Qt::SizeFDiagCursor;
        else if ((edges == (Qt::TopEdge | Qt::RightEdge)) || (edges == (Qt::BottomEdge | Qt::LeftEdge)))
            shape = Qt::SizeBDiagCursor;
        else if (edges & (Qt::LeftEdge | Qt::RightEdge))
            shape = Qt::SizeHorCursor;
        else if (edges & (Qt::TopEdge | Qt::BottomEdge))
            shape = Qt::SizeVerCursor;
        window_->setCursor(QCursor(shape));
        borderCursorActive_ = true;
    }

    bool hitInteractiveControl(QQuickItem *item, const QPointF &scenePos) const
    {
        if (!item || !item->isVisible() || !item->isEnabled() || item->opacity() <= 0.0)
            return false;
        const QPointF localPos = item->mapFromScene(scenePos);
        if (!item->contains(localPos))
            return false;
        if (item == window_->maximizeButton() || item->property("_nl_caption_hole").toBool())
            return true;
        const auto children = item->childItems();
        for (int i = children.size() - 1; i >= 0; --i) {
            if (hitInteractiveControl(children[i], scenePos))
                return true;
        }
        if (item != window_->contentItem()
            && item->acceptedMouseButtons() != Qt::NoButton) {
            return true;
        }
        return false;
    }

    NativeWindow *window_ = nullptr;
    int resizeBorderWidth_ = 6;
    bool borderCursorActive_ = false;
    bool pendingMove_ = false;
    bool manualMove_ = false;
    QPointF pressPos_;
    QPointF globalPressPos_;
    QPoint winStartPos_;
};

} // namespace

NativeWindow::NativeWindow(QWindow *parent)
    : QQuickWindow(parent)
{
    // No system title bar: the app draws its own and content fills the window.
    setFlags(flags() | Qt::FramelessWindowHint);
    setColor(QColor(0x14, 0x13, 0x11));
    // Drop the scene graph and GPU resources when the window is hidden or
    // closed to the tray instead of keeping their buffers.
    setPersistentSceneGraph(false);
    setPersistentGraphics(false);
    connect(this, &QWindow::visibilityChanged, this, [this](QWindow::Visibility) {
        emit windowVisibilityChanged();
    });
    setProperty("_nl_linux_filter", true);
    installEventFilter(new LinuxWindowEventFilter(this));
}

void NativeWindow::setDarkFrame(bool dark)
{
    if (m_darkFrame == dark)
        return;
    m_darkFrame = dark;
    applyFrame();
    emit darkFrameChanged();
}

void NativeWindow::setCaptionHeight(qreal height)
{
    if (qFuzzyCompare(m_captionHeight, height))
        return;
    m_captionHeight = height;
    emit captionHeightChanged();
}

void NativeWindow::setMaximizeButton(QQuickItem *item)
{
    if (m_maximizeButton == item)
        return;
    m_maximizeButton = item;
    emit maximizeButtonChanged();
}

void NativeWindow::addCaptionHole(QQuickItem *item)
{
    if (item)
        item->setProperty("_nl_caption_hole", true);
    if (item && std::none_of(m_holes.begin(), m_holes.end(), [item](const auto &h) { return h == item; }))
        m_holes.emplace_back(item);
}

void NativeWindow::removeCaptionHole(QQuickItem *item)
{
    if (item)
        item->setProperty("_nl_caption_hole", false);
    m_holes.erase(std::remove_if(m_holes.begin(), m_holes.end(),
                                 [item](const auto &h) { return h.isNull() || h == item; }),
                  m_holes.end());
}

void NativeWindow::toggleMaximized()
{
    if (visibility() == QWindow::Maximized)
        showNormal();
    else
        showMaximized();
}

void NativeWindow::bringToFront()
{
    if (visibility() == QWindow::Minimized) {
        setWindowStates((windowStates() & ~Qt::WindowMinimized) | Qt::WindowActive);
        show();
    } else if (!isVisible()) {
        showNormal();
    }
    raise();
    requestActivate();
    if (handle())
        activateX11Window(winId());
}

void NativeWindow::flash()
{
    if (!handle())
        return;
    alert(0);
}

void NativeWindow::releaseAndTeardown()
{
    if (QQmlEngine *engine = qmlEngine(this))
        register_qml_engine(engine);
    m_holes.clear();
    m_maximizeButton = nullptr;
    releaseResources();
    hide();
    destroy();
}

bool NativeWindow::grabToFile(const QString &path)
{
    if (path.isEmpty())
        return false;
    const QImage image = grabWindow();
    if (image.isNull())
        return false;
    return image.save(path);
}

bool NativeWindow::event(QEvent *event)
{
    if (QQmlEngine *engine = qmlEngine(this))
        register_qml_engine(engine);
    if (event->type() == QEvent::Close) {
        event->accept();
        emit closeRequested();
        return true;
    }
    if (event->type() == QEvent::PlatformSurface
        && static_cast<QPlatformSurfaceEvent *>(event)->surfaceEventType()
            == QPlatformSurfaceEvent::SurfaceCreated) {
        const bool handled = QQuickWindow::event(event);
        applyFrame();
        return handled;
    }
    return QQuickWindow::event(event);
}

void NativeWindow::applyFrame()
{
    setColor(m_darkFrame ? QColor(0x14, 0x13, 0x11) : QColor(0xF8, 0xF6, 0xF2));
    if (!property("_nl_linux_filter").toBool()) {
        setProperty("_nl_linux_filter", true);
        installEventFilter(new LinuxWindowEventFilter(this));
    }
}

bool NativeWindow::itemContains(const QQuickItem *item, const QPointF &scenePoint) const
{
    if (!item || !item->isVisible())
        return false;
    return item->mapRectToScene(QRectF(0, 0, item->width(), item->height())).contains(scenePoint);
}

bool NativeWindow::isCaptionHoleAt(const QPointF &scenePoint) const
{
    if (itemContains(m_maximizeButton, scenePoint))
        return true;
    for (const auto &hole : m_holes) {
        if (itemContains(hole, scenePoint))
            return true;
    }
    return false;
}
