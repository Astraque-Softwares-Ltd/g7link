// SPDX-License-Identifier: GPL-3.0-or-later
#include "video_view.h"

#include <QtCore/QHash>
#include <QtCore/QUrl>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtGui/QImage>
#include <QtGui/QInputMethod>
#include <QtGui/QInputMethodEvent>
#include <QtGui/QPixmap>
#include <QtGui/QVector4D>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGGeometry>
#include <QtQuick/QSGGeometryNode>
#include <QtQuick/QSGMaterial>
#include <QtQuick/QSGMaterialShader>
#include <QtQuick/QSGSimpleTextureNode>
#include <QtQuick/QSGTexture>

#include <algorithm>
#include <cstring>
#include <functional>
#include <memory>
#include <mutex>
#include <vector>

namespace {

// A decoded picture as two planes of bytes, each row padded to a multiple of
// four so a plane uploads as an RGBA texture of a quarter the width with no
// conversion (shaders/nv12.frag picks the bytes apart again).
struct Nv12Picture
{
    QByteArray luma;
    QByteArray chroma;
    int width = 0;
    int height = 0;
    // A padded row, in bytes.
    int row = 0;
    bool bt601 = false;
    bool fullRange = false;
};

struct Latest
{
    QImage image;
    std::shared_ptr<const Nv12Picture> nv12;
    quint64 serial = 0;

    QSize size() const { return nv12 ? QSize(nv12->width, nv12->height) : image.size(); }
};

// Pictures by stream, and the views to tell. Views register and leave
// under the lock, so a picture never wakes a view that's being destroyed.
// Never destroyed: a decoder thread can still be finishing while the app exits, after
// destructors of globals have run (a crash on quitting with a mirror open).
std::mutex &lock = *new std::mutex;
QHash<QString, Latest> &latest = *new QHash<QString, Latest>;
std::vector<VideoView *> &views = *new std::vector<VideoView *>;
quint64 nextSerial = 1;

void wakeViews(const QString &stream)
{
    for (VideoView *view : views) {
        if (view->stream() == stream)
            view->queueFrame();
    }
}

// Owns its texture (replaced with each picture).
class VideoNode : public QSGSimpleTextureNode
{
public:
    VideoNode() { setOwnsTexture(false); }
    ~VideoNode() override { delete texture(); }

    void replaceTexture(QSGTexture *next)
    {
        QSGTexture *previous = texture();
        setTexture(next);
        delete previous;
    }
};

// What turns Y, U and V into RGB, in 256ths: Y's scale, what V adds to red,
// U and V to green and U to blue, and Y's black level (the same numbers as
// `to_bgrx` in Rust).
struct Nv12Coefficients
{
    int y;
    int rv;
    int gu;
    int gv;
    int bu;
    int black;
};

Nv12Coefficients coefficientsOf(const Nv12Picture &picture)
{
    if (picture.bt601)
        return picture.fullRange ? Nv12Coefficients{256, 359, -88, -183, 454, 0}
                                 : Nv12Coefficients{298, 409, -100, -208, 516, 16};
    return picture.fullRange ? Nv12Coefficients{256, 403, -48, -120, 475, 0}
                             : Nv12Coefficients{298, 459, -55, -136, 541, 16};
}

// The picture as 32-bit RGB, for a scene drawn without a GPU, which can't
// run the shader (the software renderer: a virtual machine, a remote desktop).
QImage toRgb(const Nv12Picture &picture)
{
    const Nv12Coefficients c = coefficientsOf(picture);
    QImage image(picture.width, picture.height, QImage::Format_RGB32);
    if (image.isNull())
        return image;
    const auto channel = [](int value) { return std::clamp(value >> 8, 0, 255); };
    for (int y = 0; y < picture.height; ++y) {
        const auto *luma = reinterpret_cast<const uchar *>(picture.luma.constData()) + qsizetype(y) * picture.row;
        const auto *chroma =
            reinterpret_cast<const uchar *>(picture.chroma.constData()) + qsizetype(y / 2) * picture.row;
        auto *out = reinterpret_cast<QRgb *>(image.scanLine(y));
        for (int x = 0; x < picture.width; ++x) {
            const int level = c.y * (int(luma[x]) - c.black) + 128;
            const int u = int(chroma[x & ~1]) - 128;
            const int v = int(chroma[(x & ~1) + 1]) - 128;
            out[x] = qRgb(channel(level + c.rv * v), channel(level + c.gu * u + c.gv * v), channel(level + c.bu * u));
        }
    }
    return image;
}

class Nv12Material : public QSGMaterial
{
public:
    QSGMaterialType *type() const override
    {
        static QSGMaterialType type;
        return &type;
    }
    QSGMaterialShader *createShader(QSGRendererInterface::RenderMode) const override;
    // Each view draws its own picture: no two are alike enough to batch.
    int compare(const QSGMaterial *other) const override
    {
        return this == other ? 0 : (std::less<const QSGMaterial *>()(this, other) ? -1 : 1);
    }

    void setPicture(QQuickWindow *window, const std::shared_ptr<const Nv12Picture> &picture)
    {
        luma.reset(planeTexture(window, picture, picture->luma, picture->height));
        chroma.reset(planeTexture(window, picture, picture->chroma, picture->height / 2));
        sizes = QVector4D(float(picture->width), float(picture->height), float(picture->row / 4), 0);
        const Nv12Coefficients c = coefficientsOf(*picture);
        lumaColors = QVector4D(float(c.black) / 255.0f, float(c.y) / 256.0f, 0, 0);
        chromaColors = QVector4D(float(c.rv), float(c.gu), float(c.gv), float(c.bu)) / 256.0f;
    }

    std::unique_ptr<QSGTexture> luma;
    std::unique_ptr<QSGTexture> chroma;
    // The uniforms of nv12.frag: `sizes`, `luma` and `chroma`.
    QVector4D sizes;
    QVector4D lumaColors;
    QVector4D chromaColors;

private:
    // A plane's bytes as an RGBA texture. The image keeps the picture alive
    // until the render thread has uploaded it.
    static QSGTexture *planeTexture(QQuickWindow *window, const std::shared_ptr<const Nv12Picture> &picture,
                                    const QByteArray &plane, int rows)
    {
        auto *keep = new std::shared_ptr<const Nv12Picture>(picture);
        const QImage image(
            reinterpret_cast<const uchar *>(plane.constData()), picture->row / 4, rows, picture->row,
            QImage::Format_RGBA8888_Premultiplied,
            [](void *kept) { delete static_cast<std::shared_ptr<const Nv12Picture> *>(kept); }, keep);
        QSGTexture *texture = window->createTextureFromImage(image);
        if (!texture)
            return nullptr;
        // The shader blends neighbors itself: the GPU would mix bytes that
        // only share a texel.
        texture->setFiltering(QSGTexture::Nearest);
        texture->setHorizontalWrapMode(QSGTexture::ClampToEdge);
        texture->setVerticalWrapMode(QSGTexture::ClampToEdge);
        return texture;
    }
};

class Nv12Shader : public QSGMaterialShader
{
public:
    Nv12Shader()
    {
        setShaderFileName(VertexStage, QStringLiteral(":/shaders/nv12.vert.qsb"));
        setShaderFileName(FragmentStage, QStringLiteral(":/shaders/nv12.frag.qsb"));
    }

    bool updateUniformData(RenderState &state, QSGMaterial *newMaterial, QSGMaterial *) override
    {
        // std140, as in nv12.frag: mat4 at 0, float at 64, three vec4 from 80.
        QByteArray *buffer = state.uniformData();
        if (buffer->size() < 128)
            return false;
        if (state.isMatrixDirty()) {
            const QMatrix4x4 matrix = state.combinedMatrix();
            std::memcpy(buffer->data(), matrix.constData(), 64);
        }
        if (state.isOpacityDirty()) {
            const float opacity = state.opacity();
            std::memcpy(buffer->data() + 64, &opacity, 4);
        }
        const auto *material = static_cast<Nv12Material *>(newMaterial);
        std::memcpy(buffer->data() + 80, &material->sizes, 16);
        std::memcpy(buffer->data() + 96, &material->lumaColors, 16);
        std::memcpy(buffer->data() + 112, &material->chromaColors, 16);
        return true;
    }

    void updateSampledImage(RenderState &state, int binding, QSGTexture **texture, QSGMaterial *newMaterial,
                            QSGMaterial *) override
    {
        const auto *material = static_cast<Nv12Material *>(newMaterial);
        QSGTexture *plane = binding == 1 ? material->luma.get() : material->chroma.get();
        if (!plane)
            return;
        plane->commitTextureOperations(state.rhi(), state.resourceUpdateBatch());
        *texture = plane;
    }
};

QSGMaterialShader *Nv12Material::createShader(QSGRendererInterface::RenderMode) const
{
    return new Nv12Shader;
}

// Draws an NV12 picture in a rectangle.
class Nv12Node : public QSGGeometryNode
{
public:
    Nv12Node()
        : m_geometry(QSGGeometry::defaultAttributes_TexturedPoint2D(), 4)
    {
        setGeometry(&m_geometry);
        setMaterial(&m_material);
    }

    void setPicture(QQuickWindow *window, const std::shared_ptr<const Nv12Picture> &picture)
    {
        m_material.setPicture(window, picture);
        markDirty(DirtyMaterial);
    }

    bool hasPicture() const { return m_material.luma && m_material.chroma; }

    void setRect(const QRectF &rect)
    {
        if (rect == m_rect)
            return;
        m_rect = rect;
        QSGGeometry::updateTexturedRectGeometry(&m_geometry, rect, QRectF(0, 0, 1, 1));
        markDirty(DirtyGeometry);
    }

private:
    QSGGeometry m_geometry;
    Nv12Material m_material;
    QRectF m_rect;
};

} // namespace

VideoView::VideoView(QQuickItem *parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
    std::lock_guard guard(lock);
    views.push_back(this);
}

VideoView::~VideoView()
{
    std::lock_guard guard(lock);
    views.erase(std::remove(views.begin(), views.end(), this), views.end());
}

void VideoView::setStream(const QString &stream)
{
    if (stream == m_stream)
        return;
    {
        std::lock_guard guard(lock);
        m_stream = stream;
    }
    m_shown = 0;
    emit streamChanged();
    frameArrived();
}

void VideoView::setWindowIcon(const QString &icon)
{
    if (icon == m_windowIcon)
        return;
    m_windowIcon = icon;
    emit windowIconChanged();
    applyWindowChrome();
}

void VideoView::setAcceptsText(bool on)
{
    if (on == m_acceptsText)
        return;
    m_acceptsText = on;
    setFlag(ItemAcceptsInputMethod, on);
    if (hasActiveFocus())
        QGuiApplication::inputMethod()->update(Qt::ImEnabled);
    emit acceptsTextChanged();
}

void VideoView::inputMethodEvent(QInputMethodEvent *event)
{
    // Text being composed stays in the input method's own window; only
    // finished text goes to the phone.
    if (m_acceptsText && !event->commitString().isEmpty())
        emit textCommitted(event->commitString());
    event->accept();
}

QVariant VideoView::inputMethodQuery(Qt::InputMethodQuery query) const
{
    switch (query) {
    case Qt::ImEnabled:
        return m_acceptsText;
    case Qt::ImCursorRectangle: {
        // There is no caret to point at: the emoji panel and candidate
        // lists open at the middle of the phone's picture.
        const QPointF middle = pictureRect().center();
        return QRectF(middle.x(), middle.y(), 1, 1);
    }
    case Qt::ImSurroundingText:
        return QString();
    case Qt::ImCursorPosition:
    case Qt::ImAnchorPosition:
        return 0;
    default:
        return QQuickItem::inputMethodQuery(query);
    }
}

void VideoView::itemChange(ItemChange change, const ItemChangeData &value)
{
    QQuickItem::itemChange(change, value);
    if (change == ItemSceneChange && value.window) {
        connect(value.window, &QWindow::visibleChanged, this, [this](bool) { applyWindowChrome(); });
        applyWindowChrome();
    }
}

void VideoView::applyWindowChrome()
{
    QQuickWindow *win = window();
    if (!win)
        return;
    if (!m_windowIcon.isEmpty()) {
        const QUrl url(m_windowIcon);
        const QString path = url.isLocalFile() ? url.toLocalFile() : m_windowIcon;
        const QImage img(path);
        if (!img.isNull()) {
            QIcon icon;
            for (int sz : {16, 20, 24, 32, 40, 48, 64, 256}) {
                icon.addPixmap(QPixmap::fromImage(img.scaled(sz, sz, Qt::KeepAspectRatio, Qt::SmoothTransformation)));
            }
            win->setIcon(icon);
        }
    }
}

QRectF VideoView::pictureRect() const
{
    if (m_frameSize.isEmpty())
        return {};
    const QSizeF fitted = QSizeF(m_frameSize).scaled(size(), Qt::KeepAspectRatio);
    return QRectF((width() - fitted.width()) / 2, (height() - fitted.height()) / 2, fitted.width(), fitted.height());
}

void VideoView::queueFrame()
{
    // The picture cache already keeps only the newest image. Coalesce UI
    // notifications too, so a busy UI does not replay old frame callbacks.
    if (!m_framePending.exchange(true))
        QMetaObject::invokeMethod(this, "frameArrived", Qt::QueuedConnection);
}

void VideoView::frameArrived()
{
    m_framePending.store(false);
    QSize size;
    {
        std::lock_guard guard(lock);
        size = latest.value(m_stream).size();
    }
    if (size != m_frameSize) {
        m_frameSize = size;
        emit frameSizeChanged();
    }
    ++m_frames;
    emit framesChanged();
    update();
}

QSGNode *VideoView::updatePaintNode(QSGNode *old, UpdatePaintNodeData *)
{
    Latest picture;
    {
        std::lock_guard guard(lock);
        picture = latest.value(m_stream);
    }
    if ((picture.image.isNull() && !picture.nv12) || !window()) {
        delete old;
        m_shown = 0;
        return nullptr;
    }
    // Without a GPU nothing runs the shader: the picture is converted here.
    if (picture.nv12 && window()->rendererInterface()->graphicsApi() == QSGRendererInterface::Software) {
        const bool drawn = old && !m_nodeIsNv12 && picture.serial == m_shown;
        if (!drawn)
            picture.image = toRgb(*picture.nv12);
        picture.nv12.reset();
    }
    // A stream is BGRX or NV12 throughout; one that changed gets a new node.
    if (old && m_nodeIsNv12 != bool(picture.nv12)) {
        delete old;
        old = nullptr;
    }
    m_nodeIsNv12 = bool(picture.nv12);
    if (picture.nv12) {
        auto *node = static_cast<Nv12Node *>(old);
        if (!node)
            node = new Nv12Node;
        if (picture.serial != m_shown || !node->hasPicture()) {
            node->setPicture(window(), picture.nv12);
            m_shown = picture.serial;
        }
        node->setRect(pictureRect());
        return node;
    }
    auto *node = static_cast<VideoNode *>(old);
    if (!node)
        node = new VideoNode;
    if ((picture.serial != m_shown || !node->texture()) && !picture.image.isNull()) {
        QSGTexture *texture = window()->createTextureFromImage(picture.image);
        texture->setFiltering(QSGTexture::Linear);
        node->replaceTexture(texture);
        m_shown = picture.serial;
    }
    node->setFiltering(QSGTexture::Linear);
    node->setRect(pictureRect());
    return node;
}

void video_frame(rust::Str stream, uint32_t width, uint32_t height, rust::Slice<const uint8_t> bgrx)
{
    if (bgrx.size() < size_t(width) * height * 4)
        return;
    // One copy, into an image Qt owns, performed outside the shared mutex.
    QImage image(reinterpret_cast<const uchar *>(bgrx.data()), int(width), int(height), int(width) * 4,
                 QImage::Format_RGB32);
    QImage copy = image.copy();
    const QString key = QString::fromUtf8(stream.data(), qsizetype(stream.size()));
    std::lock_guard guard(lock);
    latest.insert(key, Latest{std::move(copy), nullptr, nextSerial++});
    wakeViews(key);
}

void video_frame_nv12(rust::Str stream, uint32_t width, uint32_t height, uint32_t stride, uint32_t plane_rows,
                      rust::Slice<const uint8_t> nv12, bool bt601, bool full_range)
{
    // Whole chroma samples only, and nothing a texture couldn't hold.
    const size_t w = width & ~1u;
    const size_t h = height & ~1u;
    if (w < 2 || h < 2 || w > 8192 || h > 8192 || stride < w || plane_rows < h)
        return;
    const size_t chromaStart = size_t(stride) * plane_rows;
    if (nv12.size() < chromaStart + (h / 2 - 1) * size_t(stride) + w)
        return;
    // The one copy, outside the shared mutex: each row without the decoder's
    // padding, then padded to whole texels.
    const size_t row = (w + 3) & ~size_t(3);
    auto picture = std::make_shared<Nv12Picture>();
    picture->width = int(w);
    picture->height = int(h);
    picture->row = int(row);
    picture->bt601 = bt601;
    picture->fullRange = full_range;
    picture->luma = QByteArray(qsizetype(row * h), '\0');
    picture->chroma = QByteArray(qsizetype(row * (h / 2)), '\x80');
    for (size_t y = 0; y < h; ++y)
        std::memcpy(picture->luma.data() + y * row, nv12.data() + y * stride, w);
    for (size_t y = 0; y < h / 2; ++y)
        std::memcpy(picture->chroma.data() + y * row, nv12.data() + chromaStart + y * stride, w);
    const QString key = QString::fromUtf8(stream.data(), qsizetype(stream.size()));
    std::lock_guard guard(lock);
    latest.insert(key, Latest{QImage(), std::move(picture), nextSerial++});
    wakeViews(key);
}

void video_clear(rust::Str stream)
{
    const QString key = QString::fromUtf8(stream.data(), qsizetype(stream.size()));
    std::lock_guard guard(lock);
    latest.remove(key);
    wakeViews(key);
}

void clear_idle_video_frames()
{
    std::lock_guard guard(lock);
    if (views.empty()) {
        latest.clear();
        latest.squeeze();
    }
}

