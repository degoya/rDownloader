import { driver, type Config, type DriveStep, type Driver } from 'driver.js'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import 'driver.js/dist/driver.css'

/** A step plus the route that has to be showing before its anchor exists. */
type TourStep = DriveStep & { data?: { route?: string } }

/** How long a lazily loaded route gets to render the anchor before we give up on it. */
const ANCHOR_TIMEOUT = 3000

/**
 * The guided tour offered at the end of the setup wizard.
 *
 * driver.js resolves a step's anchor before it runs any of that step's hooks, so a step living
 * on another route cannot navigate for itself. Navigation therefore happens in the click hooks,
 * which route first, wait for the anchor to actually exist, and only then move the tour on —
 * views are lazily imported, so moving before the anchor renders would silently skip the step.
 */
export function useAppTour(): { startTour: () => void } {
  const { t } = useI18n()
  const router = useRouter()

  function steps(): TourStep[] {
    const step = (
      element: string | undefined,
      key: string,
      route: string,
      side: 'top' | 'right' | 'bottom' | 'left' = 'right'
    ): TourStep => ({
      ...(element ? { element } : {}),
      popover: {
        title: t(`tour.steps.${key}.title`),
        description: t(`tour.steps.${key}.description`),
        side,
        align: 'start'
      },
      data: { route }
    })

    return [
      step('[data-tour="nav"]', 'nav', '/downloads'),
      step('[data-tour="downloads-add"]', 'downloads_add', '/downloads', 'bottom'),
      step('[data-tour="downloads-controls"]', 'downloads_controls', '/downloads', 'bottom'),
      step('[data-tour="rail"]', 'rail', '/downloads', 'top'),
      step('[data-tour="grabber-add"]', 'grabber_add', '/linkgrabber', 'bottom'),
      step('[data-tour="grabber-body"]', 'grabber_body', '/linkgrabber', 'top'),
      step('[data-tour="settings-tabs"]', 'settings', '/settings', 'bottom'),
      // No anchor: a closing note centred on the screen.
      step(undefined, 'done', '/settings')
    ]
  }

  function startTour(): void {
    const tourSteps = steps()

    /** Resolves once the step's anchor is in the DOM, or after the timeout either way. */
    function waitForAnchor(step: TourStep | undefined): Promise<void> {
      const selector = typeof step?.element === 'string' ? step.element : null
      if (!selector || document.querySelector(selector)) return Promise.resolve()
      return new Promise(resolve => {
        const deadline = Date.now() + ANCHOR_TIMEOUT
        const poll = window.setInterval(() => {
          if (document.querySelector(selector) || Date.now() > deadline) {
            window.clearInterval(poll)
            resolve()
          }
        }, 50)
      })
    }

    /** Routes to the step's page, then waits for its anchor to render. */
    async function prepare(step: TourStep | undefined): Promise<void> {
      const route = step?.data?.route
      if (route && router.currentRoute.value.path !== route) await router.push(route)
      await waitForAnchor(step)
    }

    /** Single navigation path: prepare the target step, then jump straight to it. */
    function goTo(index: number): void {
      const step = tourSteps[index]
      if (!step) {
        instance.destroy()
        return
      }
      void prepare(step).then(() => instance.moveTo(index))
    }

    const config: Config = {
      steps: tourSteps,
      showProgress: true,
      // driver.js placeholders, not vue-i18n ones: purely numeric, so no translation needed.
      progressText: '{{current}} / {{total}}',
      nextBtnText: t('tour.actions.next'),
      prevBtnText: t('tour.actions.prev'),
      doneBtnText: t('tour.actions.done'),
      popoverClass: 'rd-tour',
      onNextClick: () => goTo((instance.getActiveIndex() ?? 0) + 1),
      onPrevClick: () => goTo((instance.getActiveIndex() ?? 0) - 1)
    }

    const instance: Driver = driver(config)
    void prepare(tourSteps[0]).then(() => instance.drive())
  }

  return { startTour }
}
