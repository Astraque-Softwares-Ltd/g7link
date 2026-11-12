// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.walkthrough

/**
 * First run: the Welcome screen (what to allow, then pairing), and done.
 * Saved indexes from the older five-step walkthrough clamp to these.
 */
enum class WalkthroughStep(val index: Int, val title: String) {
    Welcome(0, "Welcome"),
    Done(1, "Setup complete"),
    ;

    val next: WalkthroughStep?
        get() = entries.getOrNull(index + 1)

    val previous: WalkthroughStep?
        get() = entries.getOrNull(index - 1)

    companion object {
        fun fromIndex(index: Int): WalkthroughStep =
            entries.firstOrNull { it.index == index.coerceIn(0, entries.lastIndex) } ?: Welcome
    }
}

/**
 * Persisted walkthrough flags stored in shared preferences.
 */
@androidx.compose.runtime.Immutable
data class WalkthroughPersistence(
    val completed: Boolean = false,
    val currentStepIndex: Int = 0,
    val whatsNewGuideDismissed: Boolean = false,
) {
    val currentStep: WalkthroughStep
        get() = WalkthroughStep.fromIndex(currentStepIndex)

    fun advanceStep(): WalkthroughPersistence {
        val nextStep = currentStep.next
        return if (nextStep != null) {
            copy(currentStepIndex = nextStep.index)
        } else {
            complete()
        }
    }

    fun previousStep(): WalkthroughPersistence {
        val prevStep = currentStep.previous ?: return this
        return copy(currentStepIndex = prevStep.index)
    }

    fun goToStep(step: WalkthroughStep): WalkthroughPersistence =
        copy(currentStepIndex = step.index)

    fun complete(): WalkthroughPersistence = copy(
        completed = true,
        currentStepIndex = 0,
        whatsNewGuideDismissed = true,
    )

    fun skip(): WalkthroughPersistence = copy(
        completed = true,
        currentStepIndex = 0,
        whatsNewGuideDismissed = true,
    )

    fun reopen(): WalkthroughPersistence = copy(
        completed = false,
        currentStepIndex = WalkthroughStep.Welcome.index,
    )

    fun dismissWhatsNewGuide(): WalkthroughPersistence = copy(
        whatsNewGuideDismissed = true,
    )
}

/**
 * Startup decision for whether to show the walkthrough or go straight to Home.
 */
@androidx.compose.runtime.Immutable
data class StartupWalkthroughResolution(
    val showWalkthrough: Boolean,
    val initialStep: WalkthroughStep,
    val markCompletedAutomatically: Boolean,
    val showWhatsNewGuideBanner: Boolean,
)

/**
 * Resolves walkthrough behavior once core state is ready:
 * - Fresh install (`!completed && !hasPairedDevices`) opens the Welcome screen.
 * - Existing paired user upgrading (`!completed && hasPairedDevices`) skips it,
 *   marks `completed = true` automatically, and shows a dismissible Guide notice on Home.
 * - Completed user (`completed == true`) stays on Home.
 */
fun resolveStartupWalkthrough(
    persistence: WalkthroughPersistence,
    hasPairedDevices: Boolean,
    walkthroughAlreadyActive: Boolean = false,
): StartupWalkthroughResolution {
    if (persistence.completed) {
        return StartupWalkthroughResolution(
            showWalkthrough = false,
            initialStep = WalkthroughStep.Welcome,
            markCompletedAutomatically = false,
            showWhatsNewGuideBanner = !persistence.whatsNewGuideDismissed && hasPairedDevices,
        )
    }
    // A first run under way stays open, even when a PC pairs through a link meanwhile.
    if (walkthroughAlreadyActive) {
        return StartupWalkthroughResolution(
            showWalkthrough = true,
            initialStep = WalkthroughStep.Welcome,
            markCompletedAutomatically = false,
            showWhatsNewGuideBanner = false,
        )
    }
    if (hasPairedDevices) {
        return StartupWalkthroughResolution(
            showWalkthrough = false,
            initialStep = WalkthroughStep.Welcome,
            markCompletedAutomatically = true,
            showWhatsNewGuideBanner = !persistence.whatsNewGuideDismissed,
        )
    }
    return StartupWalkthroughResolution(
        showWalkthrough = true,
        initialStep = WalkthroughStep.Welcome,
        markCompletedAutomatically = false,
        showWhatsNewGuideBanner = false,
    )
}
