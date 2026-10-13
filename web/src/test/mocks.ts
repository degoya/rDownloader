/**
 * The module mocks most tests repeat, as factories for `vi.mock` (RD-1280-09).
 *
 * A test file used to spell out the whole fake `@/api/client` — every verb, `responseError`,
 * `resultMessage` — by hand, and 163 copies drifted apart. A factory hands out the full shape
 * with the defaults below; a test names only what it answers differently:
 *
 *   import { apiClientMock } from '@/test/mocks'   // the first import after `vitest`
 *   vi.mock('@/api/client', () => apiClientMock({ GET: get, responseError: () => 'refused' }))
 *
 * `vi.mock` is hoisted above the imports, but the factory runs only when the mocked module is
 * first imported. Imported first, this module is ready by then; imported after a component that
 * pulls the mocked module in, the factory meets it uninitialised and the file fails at once.
 * Everything a factory is handed is read when it runs, before the file's own `const`s exist, so
 * a spy declared in the file goes through `vi.hoisted` or a wrapper (`(...args) => get(...args)`).
 *
 * This module imports nothing of the application, so it can never pull a mocked module in early.
 */
import { vi } from 'vitest'

type Verb = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
const VERBS: readonly Verb[] = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE']

/** What a test may replace in the fake `@/api/client`: a verb of `api`, or any other export. */
export type ApiClientOverrides = Partial<Record<Verb, unknown>> & Record<string, unknown>

/**
 * The fake `@/api/client`: every verb a bare `vi.fn()`, the message helpers answering as the
 * service does when it is silent. `real` is the module itself (`await importOriginal()`), for a
 * test that keeps the real message helpers and fakes only the transport.
 */
export function apiClientMock(overrides: ApiClientOverrides = {}, real: Record<string, unknown> = {}): Record<string, unknown> {
  const api: Record<string, unknown> = {}
  const rest: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(overrides)) {
    if ((VERBS as readonly string[]).includes(key)) api[key] = value
    else rest[key] = value
  }
  return {
    NETWORK_UNREACHABLE: 'network.unreachable',
    onSessionLost: vi.fn(() => () => {}),
    noticeLostSession: vi.fn(async () => {}),
    responseError: vi.fn(() => 'The service did not answer'),
    errorMessage: vi.fn(() => 'The service did not answer'),
    resultMessage: vi.fn(() => ''),
    ...real,
    ...rest,
    api: { ...Object.fromEntries(VERBS.map(verb => [verb, vi.fn()])), ...api }
  }
}

/** What a test may replace in the fake `@nuxt/ui/composables`. */
export interface ToastMockOptions {
  /** `useToast().add`; a fresh `vi.fn()` unless named. */
  add?: unknown
  /** `useToast().remove`. */
  remove?: unknown
  /** `useOverlay().create(…)`, the overlay instance a component opens. */
  overlay?: unknown
  /** `useOverlay().create` itself, for a test that tells the overlays apart by their component. */
  create?: (component: unknown) => unknown
  /** What an opened overlay (a confirm dialog) resolves to, when `overlay` is not named. */
  answer?: unknown
}

/**
 * The fake `@nuxt/ui/composables`: `useToast` with `add` and `remove`, and `useOverlay`, whose
 * overlay resolves to `answer` (`false`, the dialog dismissed, unless named).
 */
export function toastMock(options: ToastMockOptions = {}): Record<string, unknown> {
  const toast = { add: options.add ?? vi.fn(), remove: options.remove ?? vi.fn() }
  const overlay = options.overlay ?? { open: () => ({ result: Promise.resolve(options.answer ?? false) }) }
  return {
    useToast: () => toast,
    defineShortcuts: vi.fn(),
    useOverlay: () => ({ create: options.create ?? (() => overlay), overlays: [] })
  }
}

/**
 * The fake `@/composables/useConfirm`: the confirm function is `confirm`, or one that answers
 * `true` — the user agreed — and records its calls.
 */
export function confirmMock(confirm: unknown = vi.fn(async () => true)): Record<string, unknown> {
  return { useConfirm: () => confirm }
}

/**
 * The fake `@/composables/useEventStream` — jsdom has no `EventSource`: subscribing does nothing
 * and returns its release, unless a test hands in its own `subscribeEvents` to keep the handlers.
 */
export function eventStreamMock(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    subscribeEvents: () => () => {},
    onEventStreamOpened: () => () => {},
    suspendEventStream: vi.fn(),
    resumeEventStream: vi.fn(),
    resetEventStream: vi.fn(),
    ...overrides
  }
}
