// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QtCore/QSize>
#include <QtCore/QString>
#include <QtQml/qqmlregistration.h>
#include <QtQuick/QQuickItem>

#include <rust/cxx.h>

#include <atomic>
#include <cstdint>

// Shows the latest picture of a video stream (a phone's screen), scaled to
// fit with its aspect ratio kept. Pictures come from Rust, on any thread,
// through `video_frame` (BGRX) or `video_frame_nv12` (as decoded, turned
// into RGB by the GPU while it's drawn); only the newest is drawn.
class VideoView : public QQuickItem
{
    Q_OBJECT
    QML_ELEMENT

    // Which stream to show (the phone's device ID).
    Q_PROPERTY(QString stream READ stream WRITE setStream NOTIFY streamChanged)
    // The pictures' size, in pixels (empty before the first one).
    Q_PROPERTY(QSize frameSize READ frameSize NOTIFY frameSizeChanged)
    // Pictures shown so far, for a frame rate readout.
    Q_PROPERTY(quint64 frames READ frames NOTIFY framesChanged)
    // Optional PNG icon file URL/path for the containing window and taskbar button.
    Q_PROPERTY(QString windowIcon READ windowIcon WRITE setWindowIcon NOTIFY windowIconChanged)
    // Whether text from input methods (the emoji panel, IMEs for Chinese and
    // other languages) is taken, and given out through `textCommitted`.
    Q_PROPERTY(bool acceptsText READ acceptsText WRITE setAcceptsText NOTIFY acceptsTextChanged)

public:
    explicit VideoView(QQuickItem *parent = nullptr);
    ~VideoView() override;

    QString stream() const { return m_stream; }
    void setStream(const QString &stream);
    QSize frameSize() const { return m_frameSize; }
    quint64 frames() const { return m_frames; }
    QString windowIcon() const { return m_windowIcon; }
    void setWindowIcon(const QString &icon);
    bool acceptsText() const { return m_acceptsText; }
    void setAcceptsText(bool on);

    // Where the picture is drawn in the item (letterboxed).
    Q_INVOKABLE QRectF pictureRect() const;

    // Any thread, while the view registry's lock protects this view's lifetime.
    void queueFrame();

signals:
    void streamChanged();
    void frameSizeChanged();
    void framesChanged();
    void windowIconChanged();
    void acceptsTextChanged();
    // An input method finished some text (an emoji, a word in Chinese...).
    void textCommitted(const QString &text);

protected:
    QSGNode *updatePaintNode(QSGNode *old, UpdatePaintNodeData *) override;
    void itemChange(ItemChange change, const ItemChangeData &value) override;
    void inputMethodEvent(QInputMethodEvent *event) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;

private:
    Q_INVOKABLE void frameArrived();
    void applyWindowChrome();

    QString m_stream;
    QSize m_frameSize;
    quint64 m_frames = 0;
    // The serial of the picture on screen.
    quint64 m_shown = 0;
    // Whether the node drawing it is the NV12 one (it's replaced when the
    // stream changes kind).
    bool m_nodeIsNv12 = false;
    QString m_windowIcon;
    bool m_acceptsText = false;
    std::atomic_bool m_framePending{false};
};

// The latest picture of `stream`: `width` x `height`, 32-bit BGRX rows.
// Any thread.
void video_frame(rust::Str stream, uint32_t width, uint32_t height, rust::Slice<const uint8_t> bgrx);

// The latest picture of `stream` as the decoder made it: NV12, `width` x
// `height` visible in rows of `stride` bytes, the UV plane starting
// `plane_rows` rows in. Drawn without a conversion on the CPU. Any thread.
void video_frame_nv12(rust::Str stream, uint32_t width, uint32_t height, uint32_t stride, uint32_t plane_rows,
                      rust::Slice<const uint8_t> nv12, bool bt601, bool full_range);

// The stream ended: views of it go blank. Any thread.
void video_clear(rust::Str stream);

// Drops cached video frames for streams that have no live VideoView.
void clear_idle_video_frames();

