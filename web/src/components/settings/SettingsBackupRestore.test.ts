/**
 * The settings bundle (Settings → Backup): the export asks for a confirmed passphrase before it
 * includes credentials; the import checks the chosen file before anything is sent, shows what it
 * holds, wants the passphrase an encrypted bundle needs, replaces the configuration only after a
 * confirmation, and says why when any of it fails.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import settings from '@/locales/en/settings.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'
import { downloadJson } from '@/utils/jsonFile'

import SettingsBackupRestore from './SettingsBackupRestore.vue'

vi.mock('@/api/client', () => ({
  api: { POST: vi.fn() },
  responseError: vi.fn(() => 'The passphrase does not open this bundle')
}))

vi.mock('@/utils/jsonFile', async importOriginal => ({
  ...await importOriginal<typeof import('@/utils/jsonFile')>(),
  downloadJson: vi.fn()
}))

const toastAdd = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toastAdd }) }))

/** Replacing the configuration asks first; the tests drive the answer. */
const confirmed = vi.fn(async () => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const en = system.backup

const ENCRYPTED = {
  format: 'rdownloader-settings-bundle',
  version: 1,
  exported_at: '2026-10-05T10:00:00Z',
  app_version: '1.10.1',
  settings: { max_parallel_downloads: 3 },
  secrets: { salt: 'c2FsdA', nonce: 'bm9uY2U', ciphertext: 'Y2lwaGVy' }
}

const PLAIN = { ...ENCRYPTED, secrets: null }

const IMPORTED = {
  storage_roots: 1,
  categories: 2,
  category_rules: 0,
  hotfolders: 0,
  stream_channels: 1,
  proxy_profiles: 0,
  accounts: 3,
  usenet_servers: 1,
  indexers: 2
}

function renderCard() {
  return mountComponent(SettingsBackupRestore, {
    messages: { system, settings, common },
    stubs: { SettingsFullBackupCard: true, SettingsFullRestoreCard: true }
  })
}

function field(label: string): HTMLInputElement {
  return screen.getByLabelText(label) as HTMLInputElement
}

function button(name: string): HTMLButtonElement {
  return screen.getByRole('button', { name }) as HTMLButtonElement
}

/** Chooses `text` as the file in the hidden input, as the picker would. */
async function choose(text: string, name = 'rdownloader-settings.json'): Promise<void> {
  const input = document.querySelector('input[type="file"]') as HTMLInputElement
  expect(input.accept).toBe('.json')
  const file = new File([text], name, { type: 'application/json' })
  Object.defineProperty(input, 'files', { value: [file], configurable: true })
  await fireEvent.change(input)
}

const postsTo = (path: string) =>
  (vi.mocked(api.POST).mock.calls as unknown as [string, { body: Record<string, unknown> }][])
    .filter(([called]) => called === path)
    .map(([, init]) => init.body)

beforeEach(() => {
  vi.clearAllMocks()
  confirmed.mockResolvedValue(true)
})

describe('SettingsBackupRestore, export', () => {
  it('includes credentials only behind a confirmed passphrase of eight characters', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { format: 'rdownloader-settings-bundle' } } as never)
    renderCard()
    expect(button(en.export.button).disabled).toBe(true)
    await fireEvent.update(field(en.export.passphrase), 'short')
    await fireEvent.update(field(en.export.confirm_passphrase), 'short')
    expect(button(en.export.button).disabled).toBe(true)
    await fireEvent.update(field(en.export.passphrase), 'correct horse')
    await fireEvent.update(field(en.export.confirm_passphrase), 'correct horsE')
    expect(button(en.export.button).disabled).toBe(true)
    await fireEvent.update(field(en.export.confirm_passphrase), 'correct horse')
    expect(button(en.export.button).disabled).toBe(false)

    await fireEvent.click(button(en.export.button))

    await waitFor(() => expect(downloadJson).toHaveBeenCalledWith({ format: 'rdownloader-settings-bundle' }, 'settings'))
    expect(postsTo('/api/v1/settings/export')).toEqual([{ include_secrets: true, passphrase: 'correct horse' }])
    // The passphrase does not stay in the form once the file is out.
    expect(field(en.export.passphrase).value).toBe('')
    expect(field(en.export.confirm_passphrase).value).toBe('')
    expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({ title: en.export.success, color: 'success' }))
  })

  it('exports without credentials and without a passphrase when they are left out', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: {} } as never)
    renderCard()
    await fireEvent.click(screen.getByRole('switch'))
    expect(screen.queryByLabelText(en.export.passphrase)).toBeNull()
    await fireEvent.click(button(en.export.button))
    await waitFor(() => expect(postsTo('/api/v1/settings/export')).toEqual([{ include_secrets: false, passphrase: null }]))
  })

  it('says why an export failed and downloads nothing', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'internal' } } as never)
    renderCard()
    await fireEvent.click(screen.getByRole('switch'))
    await fireEvent.click(button(en.export.button))
    expect(await screen.findByText('The passphrase does not open this bundle')).toBeTruthy()
    expect(downloadJson).not.toHaveBeenCalled()
  })
})

describe('SettingsBackupRestore, import', () => {
  it('refuses a file that is no settings bundle before anything is sent', async () => {
    renderCard()
    for (const text of ['not json', JSON.stringify({ format: 'something-else', version: 1 }), JSON.stringify({ ...PLAIN, settings: null })]) {
      await choose(text)
      expect(await screen.findByText(en.import.invalid_file)).toBeTruthy()
    }
    await choose(JSON.stringify({ ...PLAIN, version: 2 }))
    expect(await screen.findByText('Bundle version 2 is not supported.')).toBeTruthy()
    expect(button(en.import.button).disabled).toBe(true)
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('shows what an encrypted bundle holds and wants its passphrase', async () => {
    renderCard()
    await choose(JSON.stringify(ENCRYPTED), 'nas-settings.json')
    expect(await screen.findByText('nas-settings.json')).toBeTruthy()
    expect(screen.getByText(en.import.encrypted)).toBeTruthy()
    expect(screen.getByText('v1 · rDownloader 1.10.1')).toBeTruthy()
    expect(button(en.import.button).disabled).toBe(true)
    await fireEvent.update(field(en.import.passphrase), 'correct horse')
    expect(button(en.import.button).disabled).toBe(false)
  })

  it('replaces nothing when the confirmation is declined', async () => {
    confirmed.mockResolvedValue(false)
    renderCard()
    await choose(JSON.stringify(PLAIN))
    await screen.findByText(en.import.without_secrets)
    await fireEvent.click(button(en.import.button))
    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('restores after the confirmation, counts what came back and clears the choice', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: IMPORTED } as never)
    const view = renderCard()
    await choose(JSON.stringify(ENCRYPTED), 'nas-settings.json')
    await fireEvent.update(await screen.findByLabelText(en.import.passphrase), 'correct horse')
    await fireEvent.click(button(en.import.button))

    await waitFor(() => expect(view.emitted('imported')).toHaveLength(1))
    expect(postsTo('/api/v1/settings/import')).toEqual([{ bundle: ENCRYPTED, passphrase: 'correct horse' }])
    expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({
      title: en.import.success,
      description: 'Imported 10 configuration entries.',
      color: 'success'
    }))
    expect(screen.queryByText('nas-settings.json')).toBeNull()
    expect(screen.queryByLabelText(en.import.passphrase)).toBeNull()
  })

  it('sends no passphrase for a bundle without credentials', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: IMPORTED } as never)
    renderCard()
    await choose(JSON.stringify(PLAIN))
    await screen.findByText(en.import.without_secrets)
    expect(screen.queryByLabelText(en.import.passphrase)).toBeNull()
    await fireEvent.click(button(en.import.button))
    await waitFor(() => expect(postsTo('/api/v1/settings/import')).toEqual([{ bundle: PLAIN, passphrase: null }]))
  })

  it('says why a restore failed and keeps the chosen bundle', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'settings.bundle_passphrase_invalid' } } as never)
    const view = renderCard()
    await choose(JSON.stringify(ENCRYPTED), 'nas-settings.json')
    await fireEvent.update(await screen.findByLabelText(en.import.passphrase), 'wrong horse')
    await fireEvent.click(button(en.import.button))
    expect(await screen.findByText('The passphrase does not open this bundle')).toBeTruthy()
    expect(screen.getByText('nas-settings.json')).toBeTruthy()
    expect(view.emitted('imported')).toBeUndefined()
    expect(toastAdd).not.toHaveBeenCalled()
  })
})
