// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QtCore/QMetaObject>
#include <QtCore/QObject>

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <memory>
#include <mutex>
#include <type_traits>
#include <utility>

/// Runs `fn` on `target`'s thread and waits for it. `fn` may refer to the caller's
/// locals, so it never runs after this returns: work that hasn't started by the
/// deadline is abandoned (returns false), and work already running is waited for.
/// Call it from another thread, one `target`'s thread doesn't wait on.
template <typename F>
bool runOnThreadAndWait(QObject *target, F &&fn, std::uint32_t timeout_ms)
{
    enum class Phase { Queued, Running, Done, Abandoned };
    struct State {
        std::mutex mutex;
        std::condition_variable cv;
        Phase phase = Phase::Queued;
    };
    auto state = std::make_shared<State>();
    auto task = std::make_shared<std::decay_t<F>>(std::forward<F>(fn));
    QMetaObject::invokeMethod(
        target,
        [state, task]() {
            {
                std::lock_guard<std::mutex> lock(state->mutex);
                if (state->phase == Phase::Abandoned)
                    return;
                state->phase = Phase::Running;
            }
            (*task)();
            {
                std::lock_guard<std::mutex> lock(state->mutex);
                state->phase = Phase::Done;
            }
            state->cv.notify_all();
        },
        Qt::QueuedConnection);

    std::unique_lock<std::mutex> lock(state->mutex);
    state->cv.wait_for(lock, std::chrono::milliseconds(timeout_ms), [&state]() {
        return state->phase == Phase::Done;
    });
    if (state->phase == Phase::Queued) {
        state->phase = Phase::Abandoned;
        return false;
    }
    state->cv.wait(lock, [&state]() { return state->phase == Phase::Done; });
    return true;
}
