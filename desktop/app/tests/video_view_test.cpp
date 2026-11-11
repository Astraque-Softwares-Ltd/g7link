// SPDX-License-Identifier: GPL-3.0-or-later
// A decoded picture (NV12) given to VideoView comes out on screen in the
// right colors, whatever padding the decoder left in its rows, and whether
// the scene is drawn by the GPU (the shader) or without one (converted in
// the view). The test runs on whichever renderer the platform gives it.
#include "video_view.h"

#include <QtCore/QTimer>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtQml/QQmlComponent>
#include <QtQml/QQmlEngine>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickView>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>

namespace {

int failures = 0;

void check(bool ok, const char *what)
{
    if (!ok) {
        std::fprintf(stderr, "FAILED: %s\n", what);
        ++failures;
    }
}

struct Yuv
{
    int y;
    int u;
    int v;
};

// BT.709 limited range, as `to_bgrx` in Rust and the shader work it out.
QColor expected(const Yuv &c)
{
    const auto channel = [](int value) { return std::clamp(value >> 8, 0, 255); };
    const int level = 298 * (c.y - 16) + 128;
    return QColor(channel(level + 459 * (c.v - 128)), channel(level - 55 * (c.u - 128) - 136 * (c.v - 128)),
                  channel(level + 541 * (c.u - 128)));
}

bool close(const QColor &a, const QColor &b)
{
    return std::abs(a.red() - b.red()) <= 2 && std::abs(a.green() - b.green()) <= 2
        && std::abs(a.blue() - b.blue()) <= 2;
}

// Four quarters of flat color, and a first row whose luma steps with x % 4:
// each byte of a row must land on its own pixel.
const Yuv QUARTERS[4] = {{81, 90, 240}, {145, 54, 34}, {41, 240, 110}, {210, 16, 146}};

std::vector<uint8_t> picture(int width, int height, int stride, int rows)
{
    std::vector<uint8_t> nv12(size_t(stride) * rows + size_t(stride) * (rows / 2 + 1), 0x55);
    for (int y = 0; y < height; ++y)
        for (int x = 0; x < width; ++x)
            nv12[size_t(y) * stride + x] = uint8_t(QUARTERS[(y >= height / 2) * 2 + (x >= width / 2)].y);
    for (int y = 0; y < height / 2; ++y) {
        for (int x = 0; x < width / 2; ++x) {
            const Yuv &c = QUARTERS[(y >= height / 4) * 2 + (x >= width / 4)];
            uint8_t *pair = &nv12[size_t(stride) * rows + size_t(y) * stride + 2 * x];
            pair[0] = uint8_t(c.u);
            pair[1] = uint8_t(c.v);
        }
    }
    for (int x = 0; x < width; ++x)
        nv12[x] = uint8_t(16 + (x % 4) * 60);
    return nv12;
}

} // namespace

int main(int argc, char **argv)
{
    QGuiApplication app(argc, argv);
    qmlRegisterType<VideoView>("app.g7link", 1, 0, "VideoView");
    // A width that isn't a multiple of four, rows longer than the picture
    // and more of them than it has: what decoders hand out.
    const int width = 322, height = 200, stride = width + 13, rows = height + 8;

    QQuickView view;
    view.setColor(Qt::black);
    view.resize(width, height);
    QQmlComponent component(view.engine());
    component.setData("import QtQuick\nimport app.g7link\nVideoView { stream: \"test\" }", QUrl());
    auto *item = qobject_cast<QQuickItem *>(component.create());
    if (!item) {
        std::fprintf(stderr, "FAILED: no VideoView: %s\n", qPrintable(component.errorString()));
        return 1;
    }
    item->setParentItem(view.contentItem());
    item->setSize(QSizeF(width, height));
    // Unshown, the window is drawn off screen on demand, by the GPU where there is one.
    if (qEnvironmentVariableIsSet("G7LINK_TEST_SHOW"))
        view.show();

    const std::vector<uint8_t> nv12 = picture(width, height, stride, rows);
    const char name[] = "test";
    const rust::Str stream(name, sizeof(name) - 1);
    video_frame_nv12(stream, width, height, stride, rows, rust::Slice<const uint8_t>(nv12.data(), nv12.size()),
                     false, false);
    // Too short for what it claims: dropped, not read past its end.
    video_frame_nv12(stream, width, height * 4, stride, rows, rust::Slice<const uint8_t>(nv12.data(), nv12.size()),
                     false, false);

    QTimer::singleShot(500, &app, [&] {
        check(item->property("frameSize").toSize() == QSize(width, height), "the view reports the picture's size");
        const QImage shot = view.grabWindow();
        std::fprintf(stderr, "renderer %d, %dx%d\n", int(view.rendererInterface()->graphicsApi()), shot.width(),
                     shot.height());
        check(shot.size() == QSize(width, height), "the window is drawn at its size");
        if (shot.size() == QSize(width, height)) {
            for (int quarter = 0; quarter < 4; ++quarter) {
                const QPoint middle((quarter % 2) * width / 2 + width / 4, (quarter / 2) * height / 2 + height / 4);
                check(close(shot.pixelColor(middle), expected(QUARTERS[quarter])), "each quarter has its color");
            }
            for (int x = 0; x < 16; ++x) {
                const QColor want = expected({16 + (x % 4) * 60, QUARTERS[0].u, QUARTERS[0].v});
                check(close(shot.pixelColor(x, 0), want), "each luma byte lands on its own pixel");
            }
        }
        video_clear(stream);
        QTimer::singleShot(100, &app, [&] {
            check(item->property("frameSize").toSize().isEmpty(), "a cleared stream has no picture");
            app.exit(failures == 0 ? 0 : 1);
        });
    });
    return app.exec();
}
