/**
 * RD-1240-34: the sizes of the update backups and the plugin cache, the question that names
 * what goes, and the clean-up sent with its confirmation as a value. RD-1240-35: the database's
 * events, archive and file beside them, the once-only rewrite and why it is refused.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsCleanupCard from './SettingsCleanupCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'This clears data for good and must be confirmed.')
}))

vi.mock('@/i18n/server', () => ({
  translateServerMessage: vi.fn(({ code }: { code: string }) => `translated ${code}`)
}))

type ConfirmOptions = { title: string, description: string, destructive?: boolean }
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const added = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: added }) }))

const MIB = 1024 * 1024

function area(keptBytes: number, removableBytes: number) {
  return { kept_files: 1, kept_bytes: keptBytes, removable_files: removableBytes ? 2 : 0, removable_bytes: removableBytes }
}

function database(removable: boolean, incremental = true, refused: string | null = null) {
  return {
    file_bytes: 564 * MIB,
    free_bytes: removable ? 3 * MIB : 0,
    incremental,
    event_rows: 1325399,
    event_bytes: 330 * MIB,
    item_rows: 169066,
    item_bytes: 190 * MIB,
    item_key_rows: 0,
    item_retention_days: 30,
    compactable_items: removable ? 1200 : 0,
    compactable_bytes: removable ? 2 * MIB : 0,
    removable_bytes: removable ? 5 * MIB : 0,
    rewrite_refused: refused
  }
}

function summary(removable: boolean, proven = true, db = database(removable)) {
  return {
    update_proven: proven,
    retention_days: 14,
    pre_update: area(500 * MIB, removable ? 1000 * MIB : 0),
    pre_migration: area(500 * MIB, removable ? 1000 * MIB : 0),
    plugin_cache: area(100 * MIB, removable ? 163 * MIB : 0),
    database: db
  }
}

function mount(model = { update_backup_retention_days: 14, subscription_item_retention_days: 30 } as Settings) {
  return mountComponent(SettingsCleanupCard, { messages: { settings }, props: { modelValue: model } })
}

beforeEach(() => {
  vi.clearAllMocks()
  confirmed.mockResolvedValue(true)
  vi.mocked(api.GET).mockResolvedValue({ data: summary(true) } as never)
  vi.mocked(api.POST).mockResolvedValue({ data: summary(true) } as never)
})

describe('SettingsCleanupCard', () => {
  it('shows each store with what of it may go', async () => {
    mount()

    await waitFor(() => expect(screen.getByTestId('cleanup-sizes')).toBeTruthy())
    const tiles = screen.getByTestId('cleanup-sizes').textContent ?? ''
    expect(tiles).toContain(settings.cleanup.pre_update)
    expect(tiles).toContain('1.5 GiB')
    expect(tiles).toContain('of which 163 MiB removable')
    expect(screen.queryByTestId('cleanup-unproven')).toBeNull()
  })

  it('names the amount in the question and sends the confirmation as a value', async () => {
    mount()
    await waitFor(() => expect((screen.getByTestId('cleanup-run') as HTMLButtonElement).disabled).toBe(false))

    await fireEvent.click(screen.getByTestId('cleanup-run'))

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    const options = confirmed.mock.calls[0]?.[0] as ConfirmOptions
    expect(options.title).toBe(settings.cleanup.confirm_title)
    expect(options.description).toContain('2.1 GiB')
    expect(options.destructive).toBe(true)
    expect(api.POST).toHaveBeenCalledWith('/api/v1/system/cleanup', { body: { confirmed: true } })
    await waitFor(() => expect(added).toHaveBeenCalled())
  })

  it('removes nothing when the question is declined', async () => {
    confirmed.mockResolvedValue(false)
    mount()
    await waitFor(() => expect((screen.getByTestId('cleanup-run') as HTMLButtonElement).disabled).toBe(false))

    await fireEvent.click(screen.getByTestId('cleanup-run'))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('is disabled with nothing to remove and says why the copies stay behind an unproven update', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: summary(false, false) } as never)
    mount()

    await waitFor(() => expect(screen.getByTestId('cleanup-unproven')).toBeTruthy())
    expect((screen.getByTestId('cleanup-run') as HTMLButtonElement).disabled).toBe(true)
  })

  it('shows the database beside them: events, the archive with what to compact, and the file', async () => {
    mount()

    await waitFor(() => expect(screen.getByTestId('cleanup-database')).toBeTruthy())
    const tiles = screen.getByTestId('cleanup-database').textContent ?? ''
    expect(tiles).toContain(settings.cleanup.events)
    expect(tiles).toContain('330 MiB')
    expect(tiles).toContain(settings.cleanup.items)
    expect(tiles).toContain('190 MiB')
    expect(tiles).toContain('1,200 old entries to compact')
    expect(tiles).toContain('564 MiB')
    expect(screen.getByTestId('cleanup-item-days')).toBeTruthy()
    expect(screen.queryByTestId('cleanup-rewrite')).toBeNull()
  })

  it('offers the once-only rewrite of an older file even with no byte promised', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: summary(false, true, database(false, false)) } as never)
    mount()

    await waitFor(() => expect(screen.getByTestId('cleanup-rewrite')).toBeTruthy())
    expect((screen.getByTestId('cleanup-run') as HTMLButtonElement).disabled).toBe(false)
  })

  it('says why the rewrite is refused and does not offer it then', async () => {
    const refused = database(false, false, 'system.cleanup_rewrite_no_space')
    vi.mocked(api.GET).mockResolvedValue({ data: summary(false, true, refused) } as never)
    mount()

    await waitFor(() => expect(screen.getByTestId('cleanup-rewrite-refused')).toBeTruthy())
    expect(screen.getByTestId('cleanup-rewrite-refused').textContent).toContain('translated system.cleanup_rewrite_no_space')
    expect(screen.queryByTestId('cleanup-rewrite')).toBeNull()
    expect((screen.getByTestId('cleanup-run') as HTMLButtonElement).disabled).toBe(true)
  })
})
