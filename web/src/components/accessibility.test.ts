/**
 * Automated accessibility checks (RD-090-11).
 *
 * axe-core finds the machine-checkable half of WCAG: missing names, bad contrast, broken
 * roles, duplicate ids. It does not find the other half — whether a keyboard can reach
 * everything, whether focus goes somewhere sensible, whether an announcement says something
 * useful — so `docs/accessibility.md` records what was checked by hand and how. A green run
 * here is a floor, not a claim of conformance.
 */
import { render } from '@testing-library/vue'
import axe from 'axe-core'
import { createPinia, setActivePinia } from 'pinia'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { LinkCandidate, Settings } from '@/api/types'

import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import settings from '@/locales/en/settings.json'
import subscriptions from '@/locales/en/subscriptions.json'
import torrent from '@/locales/en/torrent.json'

import CollectorCandidateRow from './CollectorCandidateRow.vue'
import LiveAnnouncer from './LiveAnnouncer.vue'
import VirtualRowList from './VirtualRowList.vue'
import PostprocessSteps from './PostprocessSteps.vue'
import SubscriptionItemRow from './SubscriptionItemRow.vue'
import SettingsServicesTab from './settings/SettingsServicesTab.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn()
}))
vi.mock('@/stores/transfers', () => ({
  useTransfersStore: () => ({ packages: [{ id: 'a' }, { id: 'b' }], activePackages: 1 })
}))
// The candidate row asks which providers have an account, and that lookup subscribes to
// `plugin_catalog.changed`; jsdom has no `EventSource`. Its confirmation goes through an overlay
// that only exists inside a Nuxt build.
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))

const i18n = createI18n({
  legacy: false,
  locale: 'en',
  messages: { en: { common, downloads, linkgrabber, settings, subscriptions, torrent } }
})

/** Renders slot content so what is inside a Nuxt UI wrapper is actually checked. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }

const stubs = {
  UAlert: passthrough,
  UBadge: passthrough,
  UButton: {
    props: ['label', 'ariaLabel'],
    template: '<button type="button" v-bind="$attrs" :aria-label="ariaLabel">{{ label }}<slot /></button>'
  },
  UCheckbox: {
    props: ['modelValue', 'ariaLabel'],
    template: '<input type="checkbox" v-bind="$attrs" :aria-label="ariaLabel" :checked="modelValue === true" />'
  },
  UCollapsible: passthrough,
  UDropdownMenu: passthrough,
  UFormField: passthrough,
  UIcon: { template: '<span aria-hidden="true" />' },
  UInput: { props: ['modelValue'], template: '<input v-bind="$attrs" :value="modelValue" />' },
  UProgress: { template: '<div role="progressbar" v-bind="$attrs" />' },
  USelect: { props: ['modelValue', 'items'], template: '<select v-bind="$attrs" />' },
  USwitch: {
    props: ['modelValue', 'ariaLabel', 'disabled'],
    template: '<button role="switch" v-bind="$attrs" :aria-label="ariaLabel" :aria-checked="modelValue" :disabled="disabled" />'
  },
  UTooltip: passthrough
}

/**
 * Runs axe over one rendered component.
 *
 * `region` is off: it asks every piece of content to sit inside a landmark, which is a
 * property of the page these components are mounted into, not of the components themselves —
 * the application shell provides the landmarks, and asserting it here would only be testing
 * the test harness.
 */
async function violations(container: Element): Promise<axe.Result[]> {
  const results = await axe.run(container, {
    rules: { region: { enabled: false } }
  })
  return results.violations
}

function describeViolations(found: axe.Result[]): string {
  return found
    .map(violation => `${violation.id}: ${violation.help} (${violation.nodes.length} node(s))`)
    .join('\n')
}

describe('accessibility', () => {
  it('the services settings have a name for every switch', async () => {
    const { container } = render(SettingsServicesTab, {
      props: {
        // Only the fields this component reads; the settings document has ninety more, and
        // spelling them out would say nothing about accessibility.
        modelValue: {
          torrent_service_enabled: true,
          usenet_service_enabled: true,
          media_service_enabled: false,
          gallery_service_enabled: true,
          recording_service_enabled: true,
          remote_service_enabled: true
        } as unknown as Settings
      },
      global: { plugins: [i18n], stubs }
    })
    const found = await violations(container)
    expect(describeViolations(found)).toBe('')
  })

  it('the post-processing step list is readable without sight', async () => {
    const { container } = render(PostprocessSteps, {
      props: {
        steps: [
          { owner_id: 'p', kind: 'par2', source_path: '/pkg/a.par2', state: 'completed', position: 1, output_path: null, message: null, updated_at: new Date().toISOString() },
          { owner_id: 'p', kind: 'plugin_step', source_path: 'checksums', state: 'running', position: 2, progress_percent: 40, output_path: null, message: null, updated_at: new Date().toISOString() },
          { owner_id: 'p', kind: 'script', source_path: 'done.sh', state: 'failed', position: 3, output_path: null, message: 'it did not work', updated_at: new Date().toISOString() }
        ]
      },
      global: { plugins: [i18n], stubs }
    })
    const found = await violations(container)
    expect(describeViolations(found)).toBe('')
  })

  /**
   * RD-106-08: the enlarged cover is reached through a control, not through a hover rule.
   *
   * A picture that only grows under a pointer is not there at all for somebody using a
   * keyboard or a touchscreen, so the way in is a button — which then has to have a name and
   * say whether it is open.
   */
  it('the cover in a subscription row is enlarged through a named control', async () => {
    // Inside a list, because the row is an `<li>` and axe rightly refuses a stray one. The
    // list is the caller's, exactly like the landmarks the `region` rule asks for.
    const InList = {
      components: { SubscriptionItemRow },
      data: () => ({
        item: {
          id: 'i1',
          title: 'Some.Movie.2024.1080p',
          state: 'pending',
          attributes: { coverurl: 'https://indexer.test/c.jpg', imdbscore: '7.8' }
        }
      }),
      template: '<ul><SubscriptionItemRow :item="item" :show-images="true" /></ul>'
    }
    const { container } = render(InList, { global: { plugins: [i18n], stubs } })

    const trigger = container.querySelector('button[aria-label="Show the cover larger"]')
    expect(trigger).not.toBeNull()
    expect(trigger?.getAttribute('aria-expanded')).toBe('false')
    expect(describeViolations(await violations(container))).toBe('')
  })

  /**
   * RD-106-12: a virtualized list has to say how long it is.
   *
   * Only a few dozen of a few thousand rows are in the document, so a screen reader cannot
   * count them. `role="list"` with a name carries the total, and every row says where it sits
   * in the whole (`docs/accessibility.md`).
   */
  it('a windowed list announces its real length, not the part that is rendered', async () => {
    const rows = Array.from({ length: 400 }, (_, index) => ({ key: `row-${index}`, size: 40, label: `File ${index}` }))
    const InList = {
      components: { VirtualRowList },
      data: () => ({ rows }),
      template: `
        <VirtualRowList :rows="rows" label="Download queue, 400 rows">
          <template #row="{ row }">
            <span>{{ row.label }}</span>
            <button type="button" data-row-handle aria-label="Reorder">grip</button>
          </template>
        </VirtualRowList>`
    }
    const { container } = render(InList, { global: { plugins: [i18n], stubs } })

    const list = container.querySelector('[role="list"]')
    expect(list?.getAttribute('aria-label')).toBe('Download queue, 400 rows')
    const items = container.querySelectorAll('[role="listitem"]')
    expect(items.length).toBeLessThan(400)
    expect(items[0]?.getAttribute('aria-setsize')).toBe('400')
    expect(items[0]?.getAttribute('aria-posinset')).toBe('1')
    expect(describeViolations(await violations(container))).toBe('')
  })

  /**
   * RD-110-27: the link row's controls moved into a menu and its badges into glyphs, and the
   * one way that goes wrong silently is a control or a glyph that lost its name on the way.
   */
  it('a LinkGrabber link row names every control and every glyph', async () => {
    setActivePinia(createPinia())
    const { container } = render(CollectorCandidateRow, {
      props: {
        candidate: {
          id: 'candidate-1',
          batch_id: 'batch-1',
          url: 'https://files.example.com/report.pdf',
          state: 'online',
          file_name: 'report.pdf',
          created_at: '2026-09-02T10:00:00Z',
          priority: 'normal',
          position: 1,
          size: '1048576'
        } as unknown as LinkCandidate,
        selected: false,
        busy: false
      },
      global: { plugins: [i18n], stubs }
    })
    expect(container.querySelector('button[aria-label="Link actions"]')).not.toBeNull()
    expect(describeViolations(await violations(container))).toBe('')
  })

  it('the queue announcement is a polite status region', async () => {
    const { container } = render(LiveAnnouncer, { global: { plugins: [i18n], stubs } })
    const region = container.querySelector('[role="status"]')
    expect(region).not.toBeNull()
    // Polite, never assertive: a finished download is worth knowing, not worth interrupting
    // somebody mid-sentence for.
    expect(region?.getAttribute('aria-live')).toBe('polite')
    // Read whole rather than as a diff, since the text is a summary and not a log.
    expect(region?.getAttribute('aria-atomic')).toBe('true')
    expect(region?.textContent).toContain('1 of 2')
    expect(describeViolations(await violations(container))).toBe('')
  })
})
