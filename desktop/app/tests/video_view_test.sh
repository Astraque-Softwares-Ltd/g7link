#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds VideoView on its own and checks an NV12 picture is drawn in the
# right colors. Without a display it runs on Qt's software renderer (the
# view converts the picture); with G7LINK_TEST_PLATFORM set to a platform
# that has a GPU (wayland, xcb) it runs the shader instead, off screen.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stage="$(mktemp -d -t g7link-video-view-XXXXXX)"
trap 'rm -rf "${stage}"' EXIT

if [ -n "${QT_ROOT_DIR:-}" ]; then
    qt_cflags="-I${QT_ROOT_DIR}/include"
    for module in QtCore QtGui QtQml QtQuick; do
        qt_cflags="${qt_cflags} -I${QT_ROOT_DIR}/include/${module}"
    done
    qt_libs="-L${QT_ROOT_DIR}/lib -Wl,-rpath,${QT_ROOT_DIR}/lib -lQt6Quick -lQt6Qml -lQt6Gui -lQt6Core"
    qt_tools="${QT_ROOT_DIR}/libexec"
else
    qt_cflags="$(pkg-config --cflags Qt6Quick Qt6Qml Qt6Gui Qt6Core)"
    qt_libs="$(pkg-config --libs Qt6Quick Qt6Qml Qt6Gui Qt6Core)"
    qt_tools="$(pkg-config --variable=libexecdir Qt6Core)"
fi

cat > "${stage}/shaders.qrc" <<QRC
<RCC>
  <qresource prefix="/shaders">
    <file alias="nv12.vert.qsb">${root}/shaders/nv12.vert.qsb</file>
    <file alias="nv12.frag.qsb">${root}/shaders/nv12.frag.qsb</file>
  </qresource>
</RCC>
QRC
# shellcheck disable=SC2086
"${qt_tools}/moc" ${qt_cflags} -I"${root}/tests/stubs" "${root}/cpp/video_view.h" -o "${stage}/moc_video_view.cpp"
"${qt_tools}/rcc" "${stage}/shaders.qrc" -o "${stage}/qrc_shaders.cpp"
# shellcheck disable=SC2086
g++ -std=c++17 -Wall -Wextra -Werror -Wno-deprecated-declarations -fPIC -g \
    ${qt_cflags} -I"${root}/cpp" -I"${root}/tests/stubs" \
    "${root}/tests/video_view_test.cpp" "${root}/cpp/video_view.cpp" \
    "${stage}/moc_video_view.cpp" "${stage}/qrc_shaders.cpp" ${qt_libs} -o "${stage}/test"
if [ -n "${G7LINK_TEST_PLATFORM:-}" ]; then
    QT_QPA_PLATFORM="${G7LINK_TEST_PLATFORM}" "${stage}/test"
else
    QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software "${stage}/test"
fi
