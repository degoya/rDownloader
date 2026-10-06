/**
 * Automated accessibility checks (RD-090-11).
 *
 * axe-core finds the machine-checkable half of WCAG: missing names, bad contrast, broken
 * roles, duplicate ids. It does not find the other half — whether a keyboard can reach
 * everything, whether focus goes somewhere sensible, whether an announcement says something
 * useful — so `docs/accessibility.md` records what was checked by hand and how. A green run
 * here is a floor, not a claim of conformance.
 */
import axe from 'axe-core'
import { describe, expect, it, vi } from 'vitest'

import type { Category, LinkCandidate, Settings, StorageRoot } from '@/api/types'

import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import routing from '@/locales/en/routing.json'
import settings from '@/locales/en/settings.json'
import subscriptions from '@/locales/en/subscriptions.json'
import torrent from '@/locales/en/torrent.json'
import { mountComponent } from '@/test/mount'

import CollectorCandidateRow from './CollectorCandidateRow.vue'
import LiveAnnouncer from './LiveAnnouncer.vue'
import VirtualRowList from './VirtualRowList.vue'
import PostprocessSteps from './PostprocessSteps.vue'
import RoutingCategories from './routing/RoutingCategories.vue'
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

/** The catalogues the components below read, besides `common`. */
const messages = { downloads, linkgrabber, settings, subscriptions, torrent }

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
  /**
   * RD-150-13: the categories grouped by storage root. Each section header is a button that
   * names its root and says whether it is open; the rows inside keep their named actions.
   */
  it('the categories grouped by storage root name every section and every action', async () => {
    const root = (id: string, name: string, path: string): StorageRoot =>
      ({ id, name, path, is_default: id === 'r1', minimum_free_bytes: null, persistence: 'persistent' })
    const category = (id: string, name: string, rootId: string): Category =>
      ({ id, name, color: '#38BDF8', storage_root_id: rootId, relative_path: name.toLowerCase(), is_default: id === 'c1' }) as Category
    const { container } = mountComponent(RoutingCategories, {
      messages: { routing },
      props: {
        modelValue: [category('c1', 'Films', 'r1'), category('c2', 'Series', 'r2')],
        roots: [root('r1', 'Downloads', '/downloads'), root('r2', 'Archive', '/mnt/archive')],
        loading: false,
        loadError: null
      },
      stubs: { UInputTags: true }
    })
    const headers = container.querySelectorAll('[data-accordion-item] > button')
    expect(Array.from(headers).map(header => header.getAttribute('aria-expanded'))).toEqual(['true', 'true'])
    const found = await violations(container)
    expect(describeViolations(found)).toBe('')
  })

  it('the services settings have a name for every switch', async () => {
    const { container } = mountComponent(SettingsServicesTab, {
      messages,
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
      }
    })
    const found = await violations(container)
    expect(describeViolations(found)).toBe('')
  })

  it('the post-processing step list is readable without sight', async () => {
    const { container } = mountComponent(PostprocessSteps, {
      messages,
      props: {
        steps: [
          { owner_id: 'p', kind: 'par2', source_path: '/pkg/a.par2', state: 'completed', position: 1, output_path: null, message: null, updated_at: new Date().toISOString() },
          { owner_id: 'p', kind: 'plugin_step', source_path: 'checksums', state: 'running', position: 2, progress_percent: 40, output_path: null, message: null, updated_at: new Date().toISOString() },
          { owner_id: 'p', kind: 'script', source_path: 'done.sh', state: 'failed', position: 3, output_path: null, message: 'it did not work', updated_at: new Date().toISOString() }
        ]
      }
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
    const { container } = mountComponent(InList, { messages })

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
    const { container } = mountComponent(InList, { messages })

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
    const { container } = mountComponent(CollectorCandidateRow, {
      messages,
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
      }
    })
    expect(container.querySelector('button[aria-label="Link actions"]')).not.toBeNull()
    expect(describeViolations(await violations(container))).toBe('')
  })

  it('the queue announcement is a polite status region', async () => {
    const { container } = mountComponent(LiveAnnouncer, { messages })
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
