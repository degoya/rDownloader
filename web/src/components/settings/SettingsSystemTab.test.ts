import { waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import settings from '@/locales/en/settings.json'
import stats from '@/locales/en/stats.json'
import system from '@/locales/en/system.json'
import tour from '@/locales/en/tour.json'
import usenet from '@/locales/en/usenet.json'
import wizard from '@/locales/en/wizard.json'
import { mountComponent } from '@/test/mount'

import SettingsSystemTab from './SettingsSystemTab.vue'

// Replaced at the module level rather than stubbed at render: a `stubs` entry still loads the
// real file, and that one pulls in `@nuxt/ui`'s `#imports` alias, which Vitest cannot resolve.
// The button has its own test (`SettingsDataResetButton.test.ts`); here it only has to be
// findable in the section it belongs to, carrying the count that section's confirmation names.
vi.mock('./SettingsDataResetButton.vue', () => ({
  default: {
    props: ['target', 'count'],
    template: '<div :data-testid="`data-reset-${target}`" :data-count="count" />'
  }
}))

vi.mock('@/api/client', () => ({ api: { GET: vi.fn() } }))
// The update dialog asks through useConfirm, which Nuxt UI's overlay backs; the tests never confirm.
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn() }))
// The update dialog's copy button reports a refused clipboard with a toast.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/composables/useAppTour', () => ({ useAppTour: () => ({ startTour: vi.fn() }) }))
vi.mock('@/stores/session', () => ({
  useSessionStore: () => ({ setupRequired: false, loginDisabled: false, openWizard: vi.fn() })
}))

const stubs = { SettingsReadinessCard: { template: '<div data-testid="readiness-card" />' } }

const MARKERS = '[data-testid="readiness-card"], [data-testid="system-facts"], [data-testid="log-retention"], [data-testid="audit-retention"]'

function mount() {
  const model = {
    ui_port: null,
    hotfolder_poll_seconds: 30,
    log_retention_days: 14,
    audit_retention_days: 90,
    stats_hourly_days: 30,
    stats_retention_days: 365,
    otlp_enabled: false,
    otlp_endpoint: '',
    otlp_timeout_seconds: 10
  }
  return mountComponent(SettingsSystemTab, {
    messages: { settings, stats, system, tour, usenet, wizard },
    props: { modelValue: model as never },
    stubs
  })
}

/**
 * RD-120-27: whoever opens the page wants to know what is running — version, address, poll
 * interval — before how long the service keeps things. The assertion is the order, not the
 * presence: all four sections were on the page before, in the wrong sequence.
 */
describe('SettingsSystemTab section order', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('puts the figures row between the readiness card and log retention', async () => {
    const { container } = mount()

    await waitFor(() => {
      expect(container.querySelector('[data-testid="system-facts"]')).toBeTruthy()
    })
    const order = [...container.querySelectorAll(MARKERS)].map(element => element.getAttribute('data-testid'))
    expect(order).toEqual(['readiness-card', 'system-facts', 'log-retention', 'audit-retention'])
  })

  it('moves the figures row without rebuilding it', async () => {
    const { container } = mount()

    await waitFor(() => {
      expect(container.querySelector('[data-testid="system-facts"]')).toBeTruthy()
    })
    const facts = container.querySelector('[data-testid="system-facts"]')
    expect(facts?.className).toContain('md:grid-cols-3')
    expect(facts?.className).toContain('lg:grid-cols-5')
    expect(facts?.children).toHaveLength(5)
  })
})

/**
 * RD-120-34: the three clear buttons sit in the retention section they belong to, so somebody
 * setting how long the logs are kept finds the way to empty them in the same place.
 */
describe('SettingsSystemTab clear buttons', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    // Path-aware: the tab loads agents, servers and the version as lists, and the counts as
    // one object. One blanket answer makes the other three loads throw.
    vi.mocked(api.GET).mockImplementation((async (path: string) => (
      path === '/api/v1/system/data-reset' ? { data: { logs: 41, audit: 9, stats: 3 } } : { data: [] }
    )) as never)
  })

  it.each([
    ['log-retention', 'data-reset-logs'],
    ['audit-retention', 'data-reset-audit'],
    ['stats-retention', 'data-reset-stats']
  ])('puts the clear button inside %s', async (section, button) => {
    const { container } = mount()

    await waitFor(() => {
      expect(container.querySelector(`[data-testid="${button}"]`)).toBeTruthy()
    })
    expect(container.querySelector(`[data-testid="${section}"] [data-testid="${button}"]`)).toBeTruthy()
  })

  it('hands each button the count its confirmation has to name', async () => {
    const { container } = mount()

    // The counts arrive after the section does, so the wait is for the figure, not the button.
    await waitFor(() => {
      expect(container.querySelector('[data-testid="data-reset-logs"]')?.getAttribute('data-count')).toBe('41')
    })
    expect(container.querySelector('[data-testid="data-reset-audit"]')?.getAttribute('data-count')).toBe('9')
    expect(container.querySelector('[data-testid="data-reset-stats"]')?.getAttribute('data-count')).toBe('3')
  })
})

/** RD-1120-23: the import history is kept or dropped with the other retention rules, not on General. */
describe('SettingsSystemTab import history', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('carries the switch on the retention tab, after the other retention cards', async () => {
    const { container } = mount()

    await waitFor(() => {
      expect(container.querySelector('[data-testid="import-history-retention"]')).toBeTruthy()
    })
    const card = container.querySelector('[data-testid="import-history-retention"]') as HTMLElement
    expect(card.closest('[data-tab="retention"]')).not.toBeNull()
    expect(card.querySelector('[data-settings-anchor="system.import_history"]')?.textContent).toContain(settings.import_history.label)
    const order = [...container.querySelectorAll('[data-testid$="-retention"]')].map(element => element.getAttribute('data-testid'))
    expect(order.at(-1)).toBe('import-history-retention')
  })
})
