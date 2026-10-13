/**
 * The shared component-test harness (RD-104-07).
 *
 * Every component test in this project used to repeat the same three things by hand: an
 * `createI18n` with a hand-picked set of catalogues, a fresh Pinia, and a map of Nuxt UI
 * stubs — because the real components resolve through `#imports`, which only exists inside a
 * Nuxt build. Twenty copies of that map drift apart, and a test then fails for a missing stub
 * rather than for the behaviour it was written to check.
 *
 * `vitest.config.ts` has no `setupFiles`, and this is deliberately not one: a setup file
 * cannot hand a test the catalogues it needs, and a global stub registry would hide which
 * component a test actually depends on. It is a module a test imports.
 */
import { render, type RenderResult } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { vi } from 'vitest'
import { nextTick, type Component } from 'vue'
import { createI18n } from 'vue-i18n'

import { messageResolver } from '@/i18n/resolver'
import { DATETIME_FORMATS } from '@/i18n/formats'
import common from '@/locales/en/common.json'

import { uiStubs } from './stubs'

export { fileUpload, listViewStubs, modelInput, openablePopover, passthrough, uiStubs, unitOf } from './stubs'

/**
 * An i18n instance over the English catalogues a test names. `common` is always present
 * because the shared states — loading, failed — live there.
 */
export function createTestI18n(messages: Record<string, unknown> = {}, locale = 'en') {
  return createI18n({
    legacy: false,
    locale,
    messages: { [locale]: { common, ...messages } },
    // The application's resolver, not vue-i18n's: a backend code is itself dotted
    // (`server.codes.site_rules.structure`) and is stored as a literal key, so a test with the
    // default resolver would see every such message fall back to its key while the running
    // application resolves it.
    messageResolver,
    // The same shapes the application registers. Without them `d()` answers an unregistered
    // format with an empty string, so a timestamp a component renders would vanish under test
    // and the test would still pass.
    datetimeFormats: { [locale]: DATETIME_FORMATS }
  })
}

interface MountOptions {
  /** Locale catalogues besides `common`, keyed by their namespace (`settings`, `network`, …). */
  messages?: Record<string, unknown>
  props?: Record<string, unknown>
  /** Extra or replacement stubs on top of `uiStubs`. */
  stubs?: Record<string, unknown>
  /** Extra plugins, for the rare test that needs a router. */
  plugins?: unknown[]
  /**
   * The locale the catalogues are registered under, `en` unless a test says otherwise.
   *
   * A test that has to prove a screen reads in somebody's own language needs the component
   * rendered in it, not the catalogue compared against itself (RD-120-15).
   */
  locale?: string
  /** An i18n instance the test built itself, to switch its locale later; replaces `messages` and `locale`. */
  i18n?: unknown
  /**
   * Renders against the Pinia the test made active and filled first, instead of a fresh one
   * (RD-1280-09) — for a view whose stores are seeded before it mounts.
   */
  keepPinia?: boolean
}

/**
 * Renders a component with a fresh Pinia, an i18n instance and the shared stubs.
 *
 * The Pinia is created per mount rather than per file, so a store a test writes to cannot
 * leak into the next one.
 */
export function mountComponent(component: Component, options: MountOptions = {}): RenderResult {
  if (!options.keepPinia) setActivePinia(createPinia())
  return render(component, {
    props: options.props,
    global: {
      plugins: [options.i18n ?? createTestI18n(options.messages, options.locale), ...(options.plugins ?? [])] as never[],
      stubs: { ...uiStubs, ...options.stubs } as never
    }
  })
}

/** The timer of the page as this module found it, so `settle` still runs under `vi.useFakeTimers`. */
const realSetTimeout = globalThis.setTimeout

/**
 * Lets a handler chain — a mocked answer, a store write, the re-render, a focus restore — finish:
 * every promise already queued, then Vue's next render (RD-1280-09; twelve files kept a copy).
 * It waits one turn of the real event loop, so a test on fake timers advances those itself.
 */
export async function settle(): Promise<void> {
  await new Promise(resolve => realSetTimeout(resolve, 0))
  await nextTick()
}

/**
 * Runs `action` — a keystroke, a mount, an event — on fake timers and lets `ms` of them pass, for
 * a debounce a test used to sit out in real time (RD-1280-09: six waits of 220 to 400 ms). Real
 * timers are back afterwards, since Testing Library's `waitFor` cannot run on Vitest's fake ones;
 * a timer still pending then is dropped with them.
 */
export async function passTime(ms: number, action?: () => unknown): Promise<void> {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
  try {
    await action?.()
    await vi.advanceTimersByTimeAsync(ms)
  } finally {
    vi.useRealTimers()
  }
}
