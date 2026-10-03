/**
 * The indexers defined once under Settings › Usenet (RD-180-19): the key goes out once and is
 * never asked back, an edit without it keeps it, and the test is `t=caps` with the stored key.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import usenet from '@/locales/en/usenet.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
const del = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: (...args: unknown[]) => put(...args),
    PATCH: vi.fn(),
    DELETE: (...args: unknown[]) => del(...args)
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))

const { default: SettingsIndexersCard } = await import('./SettingsIndexersCard.vue')

const STORED = {
  id: 'idx-1',
  name: 'Omg',
  url: 'https://api.example.test/api',
  has_secret: true,
  categories: ['5040'],
  enabled: true,
  created_at: '2026-10-01T08:00:00Z',
  updated_at: '2026-10-01T08:00:00Z'
}

function mount() {
  return mountComponent(SettingsIndexersCard, {
    messages: { usenet },
    stubs: { UInputTags: { props: ['modelValue'], emits: ['update:modelValue'], template: '<input v-bind="$attrs" :value="(modelValue ?? []).join(\',\')" @input="$emit(\'update:modelValue\', $event.target.value.split(\',\').filter(Boolean))" />' } }
  })
}

describe('SettingsIndexersCard', () => {
  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    put.mockReset()
    del.mockReset()
  })

  it('defines an indexer with its key once and lists it without the key', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: STORED })
    const { container } = mount()
    await screen.findByText(usenet.indexers.empty)

    await fireEvent.update(screen.getByTestId('indexer-name'), ' Omg ')
    await fireEvent.update(screen.getByTestId('indexer-url'), 'https://api.example.test/api')
    await fireEvent.update(screen.getByTestId('indexer-api-key'), 'secret-key')
    await fireEvent.update(screen.getByTestId('indexer-categories'), '5040')
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/indexers', {
      body: { name: 'Omg', url: 'https://api.example.test/api', api_key: 'secret-key', categories: ['5040'], enabled: true, list_style: 'compact' }
    }))
    expect(await screen.findAllByTestId('indexer-row')).toHaveLength(1)
    // The form is empty again; the key is not kept anywhere in the page.
    expect((screen.getByTestId('indexer-api-key') as HTMLInputElement).value).toBe('')
    expect(container.innerHTML).not.toContain('secret-key')
  })

  it('keeps the stored key when an edit leaves the field empty', async () => {
    get.mockResolvedValue({ data: [STORED] })
    put.mockResolvedValue({ data: { ...STORED, name: 'Renamed' } })
    const { container } = mount()
    await screen.findAllByTestId('indexer-row')

    await fireEvent.click(screen.getByRole('button', { name: usenet.indexers.edit }))
    expect((screen.getByTestId('indexer-api-key') as HTMLInputElement).placeholder).toBe(usenet.indexers.api_key_keep)
    await fireEvent.update(screen.getByTestId('indexer-name'), 'Renamed')
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(put).toHaveBeenCalled())
    const [path, request] = put.mock.calls[0] as [string, { params: { path: { id: string } }, body: Record<string, unknown> }]
    expect(path).toBe('/api/v1/indexers/{id}')
    expect(request.params.path.id).toBe('idx-1')
    expect(request.body).toMatchObject({ name: 'Renamed', api_key: null, categories: ['5040'] })
  })

  it('offers the list style, compact for a new indexer, and sends the chosen one (RD-190-16)', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { ...STORED, list_style: 'detailed' } })
    const { container } = mount()
    await screen.findByText(usenet.indexers.empty)

    const compact = screen.getByLabelText(usenet.indexers.list_styles.compact) as HTMLInputElement
    const detailed = screen.getByLabelText(usenet.indexers.list_styles.detailed) as HTMLInputElement
    expect(compact.checked).toBe(true)
    expect(detailed.checked).toBe(false)

    await fireEvent.update(screen.getByTestId('indexer-name'), 'Omg')
    await fireEvent.update(screen.getByTestId('indexer-url'), 'https://api.example.test/api')
    await fireEvent.update(screen.getByTestId('indexer-api-key'), 'secret-key')
    await fireEvent.click(detailed)
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/indexers', {
      body: expect.objectContaining({ list_style: 'detailed' })
    }))
    // The row says which indexer draws its hits in detail.
    expect((await screen.findByTestId('indexer-detailed')).textContent).toContain(usenet.indexers.list_styles.detailed)
    // The form starts over compact.
    expect((screen.getByLabelText(usenet.indexers.list_styles.compact) as HTMLInputElement).checked).toBe(true)
  })

  it('edits an indexer with its stored list style and sends it back unchanged', async () => {
    get.mockResolvedValue({ data: [{ ...STORED, list_style: 'detailed' }] })
    put.mockResolvedValue({ data: { ...STORED, list_style: 'detailed' } })
    const { container } = mount()
    await screen.findAllByTestId('indexer-row')

    await fireEvent.click(screen.getByRole('button', { name: usenet.indexers.edit }))
    expect((screen.getByLabelText(usenet.indexers.list_styles.detailed) as HTMLInputElement).checked).toBe(true)
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(put).toHaveBeenCalled())
    const [, request] = put.mock.calls[0] as [string, { body: Record<string, unknown> }]
    expect(request.body.list_style).toBe('detailed')
  })

  it('tests a stored indexer with t=caps and says what it answered', async () => {
    get.mockResolvedValue({ data: [STORED] })
    post.mockResolvedValue({ data: { server: 'Omg API', searching: ['search'], categories: [{ id: '5000', name: 'TV' }, { id: '5040', name: 'HD', parent_id: '5000' }] } })
    mount()
    await screen.findAllByTestId('indexer-row')

    await fireEvent.click(screen.getByTestId('indexer-test-idx-1'))

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/indexers/{id}/caps', { params: { path: { id: 'idx-1' } } }))
    expect((await screen.findByTestId('indexer-message')).textContent).toContain('Omg API')
  })

  it('asks a new indexer for its categories with the typed key, which it does not store', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { server: null, searching: [], categories: [{ id: '2000', name: 'Movies' }] } })
    mount()
    await screen.findByText(usenet.indexers.empty)
    const load = screen.getByTestId('indexer-load-categories') as HTMLButtonElement
    expect(load.disabled).toBe(true)

    await fireEvent.update(screen.getByTestId('indexer-url'), 'https://api.example.test/api')
    await fireEvent.update(screen.getByTestId('indexer-api-key'), 'secret-key')
    await fireEvent.click(load)

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/subscriptions/caps', {
      body: { url: 'https://api.example.test/api', api_key: 'secret-key' }
    }))
  })
})
