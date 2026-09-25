/**
 * The log viewer draws what the store holds and the bundle is only offered after a preview
 * (RD-110-02). The redaction itself is a server-side promise, tested where it is kept.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import logsDe from '@/locales/de/logs.json'
import logs from '@/locales/en/logs.json'
import { mountComponent } from '@/test/mount'

/** Every sentence the bundle's codes render to, in one language. */
function sentences(group: unknown): string[] {
  if (typeof group === 'string') return [group]
  if (typeof group !== 'object' || group === null) return []
  return Object.values(group as Record<string, unknown>).flatMap(sentences)
}

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))

const PAGE = {
  records: [
    { id: 2, recorded_at: '2026-09-20T10:00:01.000Z', level: 'error', component: 'rd_http::engine', code: 'http.status', correlation_id: 'dl-1', message: 'transfer failed', fields: { url: 'https://h.example/f?token=%5Bredacted%5D' } },
    { id: 1, recorded_at: '2026-09-20T10:00:00.000Z', level: 'info', component: 'rd_scheduler', code: null, correlation_id: null, message: 'queue started', fields: {} }
  ],
  full_page: true,
  total: 40,
  captured: 40,
  dropped: 0,
  retention: { records: 20000, days: 14 }
}

/**
 * What the server sends: stable codes plus the English rendering the archive keeps, never a
 * finished sentence (RD-120-15). `params` carries data -- the field names that were replaced --
 * which no catalogue translates.
 */
const PREVIEW = {
  entries: [
    {
      id: 'versions',
      path: 'versions.json',
      kind: 'json',
      description: { code: 'diagnostics.bundle.entry.versions.description', text: logs.diagnostics.bundle.entry.versions.description },
      items: 3,
      redactions: []
    },
    {
      id: 'configuration',
      path: 'configuration.json',
      kind: 'json',
      description: { code: 'diagnostics.bundle.entry.configuration.description', text: logs.diagnostics.bundle.entry.configuration.description },
      items: 40,
      redactions: [
        { code: 'diagnostics.bundle.redaction.configuration_pem', text: logs.diagnostics.bundle.redaction.configuration_pem },
        { code: 'diagnostics.bundle.redaction.configuration_replaced', text: 'replaced: admin_password, api_key', params: { fields: 'admin_password, api_key' } }
      ]
    }
  ],
  excluded: [{ code: 'diagnostics.bundle.exclusion.secrets', text: logs.diagnostics.bundle.exclusion.secrets }],
  digest: 'digest-1',
  directory: '/data/diagnostics'
}

async function mountView(locale = 'en') {
  const { default: LogsView } = await import('./LogsView.vue')
  const catalogue = locale === 'de' ? logsDe : logs
  return mountComponent(LogsView, { messages: { logs: catalogue }, locale })
}

beforeEach(() => {
  get.mockReset()
  post.mockReset()
  get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/diagnostics/logs') return { data: PAGE }
    if (path === '/api/v1/diagnostics/bundle/preview') return { data: PREVIEW }
    return { error: { code: 'internal.error' } }
  })
})

describe('LogsView', () => {
  it('draws the records with their level, code, correlation and message, and says the page is full', async () => {
    await mountView()

    const list = await screen.findByTestId('log-list')
    expect(within(list).getByText('transfer failed')).toBeTruthy()
    expect(within(list).getByText('http.status')).toBeTruthy()
    expect(within(list).getByText('dl-1')).toBeTruthy()
    expect(within(list).getByText('Error')).toBeTruthy()
    expect(screen.getByText('2 of 40 records')).toBeTruthy()
    expect(screen.getByText(logs.list.full_page)).toBeTruthy()
    expect(screen.getByRole('button', { name: logs.list.older })).toBeTruthy()
  })

  it('opens the fields of a record behind the chevron, never inline', async () => {
    await mountView()
    const list = await screen.findByTestId('log-list')
    expect(screen.queryByTestId('log-fields')).toBeNull()

    const toggles = within(list).getAllByRole('button', { name: logs.list.expand })
    expect(toggles).toHaveLength(1)
    await fireEvent.click(toggles[0]!)

    const fields = await screen.findByTestId('log-fields')
    expect(within(fields).getByText('url')).toBeTruthy()
    expect(within(fields).getByText('https://h.example/f?token=%5Bredacted%5D')).toBeTruthy()
    expect(within(list).getByRole('button', { name: logs.list.collapse }).getAttribute('aria-expanded')).toBe('true')
  })

  it('applies the filters through the store', async () => {
    await mountView()
    await screen.findByTestId('log-list')

    await fireEvent.update(screen.getByTestId('log-search'), 'failed')
    await fireEvent.click(screen.getByRole('button', { name: logs.filters.apply }))

    await waitFor(() => {
      expect(get).toHaveBeenLastCalledWith('/api/v1/diagnostics/logs', {
        params: { query: { limit: 200, search: 'failed' } }
      })
    })
  })

  it('offers the bundle only after its preview was shown, and sends what was ticked', async () => {
    post.mockResolvedValue({ data: { file_name: 'rdownloader-diagnostics-20260920T120000Z.zip', path: '/data/diagnostics/rdownloader-diagnostics-20260920T120000Z.zip', bytes: 1234, manifest: {} } })
    await mountView()
    await screen.findByTestId('log-list')

    const create = screen.getByRole('button', { name: logs.bundle.create }) as HTMLButtonElement
    expect(create.disabled).toBe(true)
    expect(screen.queryByTestId('bundle-preview')).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: logs.bundle.preview }))
    const preview = await screen.findByTestId('bundle-preview')
    expect(within(preview).getByText('versions.json')).toBeTruthy()
    expect(within(preview).getByText(logs.diagnostics.bundle.exclusion.secrets)).toBeTruthy()
    expect(within(preview).getByText(/replaced: admin_password, api_key/)).toBeTruthy()
    await waitFor(() => expect(create.disabled).toBe(false))

    const boxes = within(preview).getAllByRole('checkbox') as HTMLInputElement[]
    expect(boxes.every(box => box.checked)).toBe(true)
    await fireEvent.click(boxes[1]!)
    await fireEvent.click(create)

    await waitFor(() => {
      expect(post).toHaveBeenCalledWith('/api/v1/diagnostics/bundle', {
        body: { approved: true, digest: 'digest-1', entries: ['versions'] }
      })
    })
    const created = await screen.findByTestId('bundle-created')
    expect(within(created).getByText('Bundle written: rdownloader-diagnostics-20260920T120000Z.zip')).toBeTruthy()
    expect(within(created).getByRole('button', { name: logs.bundle.download })).toBeTruthy()
  })

  // The finding this closes: the frame was German and everything under it English, including
  // the lines that say what was redacted (RD-120-15).
  it('draws the preview in the reader\u2019s language, with no English sentence under the German frame', async () => {
    await mountView('de')
    await screen.findByTestId('log-list')

    await fireEvent.click(screen.getByRole('button', { name: logsDe.bundle.preview }))
    const preview = await screen.findByTestId('bundle-preview')

    for (const german of [
      logsDe.diagnostics.bundle.entry.versions.description,
      logsDe.diagnostics.bundle.entry.configuration.description,
      logsDe.diagnostics.bundle.redaction.configuration_pem,
      logsDe.diagnostics.bundle.exclusion.secrets
    ]) {
      expect(preview.textContent).toContain(german)
    }
    // The field names are data and stay as they are; the sentence around them is German.
    expect(preview.textContent).toContain('ersetzt: admin_password, api_key')

    for (const english of sentences(logs.diagnostics)) {
      expect(preview.textContent).not.toContain(english)
    }
  })

  it('falls back to the English the server sent rather than showing a raw code', async () => {
    get.mockImplementation(async (path: string) => {
      if (path === '/api/v1/diagnostics/logs') return { data: PAGE }
      if (path === '/api/v1/diagnostics/bundle/preview') {
        return {
          data: {
            ...PREVIEW,
            excluded: [{ code: 'diagnostics.bundle.exclusion.not_in_any_catalogue', text: 'a line no catalogue knows yet' }]
          }
        }
      }
      return { error: { code: 'internal.error' } }
    })
    await mountView('de')
    await screen.findByTestId('log-list')

    await fireEvent.click(screen.getByRole('button', { name: logsDe.bundle.preview }))
    const preview = await screen.findByTestId('bundle-preview')
    expect(preview.textContent).toContain('a line no catalogue knows yet')
    expect(preview.textContent).not.toContain('diagnostics.bundle.exclusion.not_in_any_catalogue')
  })
})
