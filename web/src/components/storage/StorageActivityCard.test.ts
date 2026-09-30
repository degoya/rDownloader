/**
 * RD-180-13: the storage card offers emptying its history beside the history and emptying the
 * content index beside its check, each with the count a clear would take -- which is not the
 * length of the list: a running operation stays, and the list shows only the newest fifty.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

const history = vi.fn()
const check = vi.fn()
vi.mock('@/api/storage', async (original) => ({
  ...(await original<typeof import('@/api/storage')>()),
  listStorageOperations: () => history(),
  listReuseCapabilities: async () => ({ ok: true, data: [] }),
  listLinkSupport: async () => ({ ok: true, data: [] }),
  checkContentIndex: () => check()
}))

// The button has its own test (`SettingsDataResetButton.test.ts`); here it only has to sit at
// the right place, carry the count, and be able to say it cleared.
vi.mock('@/components/settings/SettingsDataResetButton.vue', () => ({
  default: {
    props: ['target', 'count'],
    emits: ['cleared'],
    template: '<button :data-testid="`data-reset-${target}`" :data-count="count" @click="$emit(\'cleared\')" />'
  }
}))

const { default: StorageActivityCard } = await import('./StorageActivityCard.vue')

function operation(id: number, state: string) {
  return {
    id,
    kind: 'move',
    state,
    package_id: null,
    download_id: null,
    source_path: `/old/${id}.bin`,
    target_path: `/new/${id}.bin`,
    size_bytes: null,
    verified_digest: null,
    error_code: null,
    error_message: null,
    started_at: '2026-09-30T10:00:00Z',
    finished_at: null
  }
}

function counts(storageOperations: number, contentIndex: number): void {
  get.mockImplementation(async (path: string) =>
    path === '/api/v1/system/data-reset'
      ? { data: { logs: 0, audit: 0, stats: 0, notifications: 0, notifications_pending: 0, storage_operations: storageOperations, content_index: contentIndex } }
      : { data: null }
  )
}

describe('StorageActivityCard clearing', () => {
  beforeEach(() => {
    get.mockReset()
    history.mockReset()
    check.mockReset()
  })

  it('offers both clears with the count each would take', async () => {
    history.mockResolvedValue({ ok: true, data: [operation(2, 'running'), operation(1, 'completed')] })
    counts(1, 40)

    mountComponent(StorageActivityCard, { messages: { settings } })

    await screen.findByText('/old/1.bin → /new/1.bin')
    await waitFor(() => expect(screen.getByTestId('data-reset-storage_operations').getAttribute('data-count')).toBe('1'))
    expect(screen.getByTestId('data-reset-content_index').getAttribute('data-count')).toBe('40')
    // The index clear sits with the check, not with the history.
    const indexActions = screen.getByTestId('storage-index-actions')
    expect(indexActions.textContent).toContain('Check the content index')
    expect(indexActions.contains(screen.getByTestId('data-reset-content_index'))).toBe(true)
  })

  it('reads the history and the count again once the history was cleared', async () => {
    history.mockResolvedValue({ ok: true, data: [operation(2, 'running'), operation(1, 'completed')] })
    counts(1, 40)
    mountComponent(StorageActivityCard, { messages: { settings } })
    await screen.findByText('/old/1.bin → /new/1.bin')

    history.mockResolvedValue({ ok: true, data: [operation(2, 'running')] })
    counts(0, 40)
    await fireEvent.click(screen.getByTestId('data-reset-storage_operations'))

    await waitFor(() => expect(screen.queryByText('/old/1.bin → /new/1.bin')).toBeNull())
    expect(screen.getByText('/old/2.bin → /new/2.bin')).toBeTruthy()
    await waitFor(() => expect(screen.getByTestId('data-reset-storage_operations').getAttribute('data-count')).toBe('0'))
  })

  it('drops an old check result and reads the index count again once the index was cleared', async () => {
    history.mockResolvedValue({ ok: true, data: [] })
    check.mockResolvedValue({ ok: true, data: { checked: 40, missing: 0, restored: 0, backfilled: 0 } })
    counts(0, 40)
    mountComponent(StorageActivityCard, { messages: { settings } })
    await fireEvent.click(await screen.findByText('Check the content index'))
    await screen.findByText(/Content index checked: 40 entries/)

    counts(0, 0)
    await fireEvent.click(screen.getByTestId('data-reset-content_index'))

    await waitFor(() => expect(screen.queryByText(/Content index checked/)).toBeNull())
    await waitFor(() => expect(screen.getByTestId('data-reset-content_index').getAttribute('data-count')).toBe('0'))
  })
})
