#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stage="$(mktemp -d -t g7link-thread-sync-XXXXXX)"
trap 'rm -rf "${stage}"' EXIT

if [ -n "${QT_ROOT_DIR:-}" ]; then
    qt_cflags="-I${QT_ROOT_DIR}/include -I${QT_ROOT_DIR}/include/QtCore"
    qt_libs="-L${QT_ROOT_DIR}/lib -Wl,-rpath,${QT_ROOT_DIR}/lib -lQt6Core"
else
    qt_cflags="$(pkg-config --cflags Qt6Core)"
    qt_libs="$(pkg-config --libs Qt6Core)"
fi

# shellcheck disable=SC2086
g++ -std=c++17 -Wall -Wextra -Werror -fPIC -g -fsanitize=address,undefined \
    ${qt_cflags} "${root}/tests/qt_thread_sync_test.cpp" ${qt_libs} -pthread -o "${stage}/test"
ASAN_OPTIONS=detect_leaks=0 QT_QPA_PLATFORM=offscreen "${stage}/test"
