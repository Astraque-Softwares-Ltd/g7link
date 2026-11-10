// SPDX-License-Identifier: GPL-3.0-or-later
// runOnThreadAndWait must never run a task after it has returned: the task
// writes to the caller's locals, and those are gone by then.
#include "../cpp/qt_thread_sync.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QThread>

#include <atomic>
#include <cstdio>
#include <thread>

namespace {

int failures = 0;

void check(bool ok, const char *what)
{
    if (!ok) {
        std::fprintf(stderr, "FAILED: %s\n", what);
        ++failures;
    }
}

// Lets `target`'s queued work run, on this (its) thread, for a while.
void pump(int ms)
{
    const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    while (std::chrono::steady_clock::now() < until) {
        QCoreApplication::processEvents();
        QThread::msleep(2);
    }
}

} // namespace

int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);

    // The event loop is free: the task runs and its result is there.
    {
        int value = 0;
        bool returned = false;
        std::thread worker([&] {
            returned = runOnThreadAndWait(&app, [&value] { value = 7; }, 2000);
        });
        pump(100);
        worker.join();
        check(returned, "a task on a free event loop reports success");
        check(value == 7, "a task on a free event loop has run");
    }

    // The event loop is busy past the deadline: the task must never run afterwards.
    {
        std::atomic<int> ran{0};
        std::atomic<bool> returned{true};
        std::thread worker([&] {
            returned = runOnThreadAndWait(&app, [&ran] { ran.fetch_add(1); }, 30);
        });
        QThread::msleep(150);
        worker.join();
        pump(100);
        check(!returned, "a task that never started reports failure");
        check(ran == 0, "a task abandoned at its deadline never runs");
    }

    // The task outlasts the deadline: the caller waits, so its locals stay valid.
    {
        std::atomic<bool> finished{false};
        std::atomic<bool> finishedWhenReturned{false};
        std::atomic<bool> returned{false};
        std::thread worker([&] {
            returned = runOnThreadAndWait(
                &app,
                [&finished] {
                    QThread::msleep(120);
                    finished = true;
                },
                20);
            finishedWhenReturned = finished.load();
        });
        pump(250);
        worker.join();
        check(returned, "a task that started in time reports success");
        check(finishedWhenReturned, "the caller waits for a task that's already running");
    }

    if (failures == 0)
        std::puts("qt_thread_sync: ok");
    return failures == 0 ? 0 : 1;
}
