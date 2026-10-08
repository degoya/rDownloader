/**
 * The object storage card (RD-150-04, RD-150-05): the provider drives the form and comes first,
 * the credential source next; a stored secret is never read back, a test answer stands above the
 * form in words, and a delete asks first.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import remote from '@/locales/en/remote.json'
import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'

import SettingsObjectStorageCard from './SettingsObjectStorageCard.vue'

const en = remote.object_storage

const PROFILE = {
  id: 'p1', name: 'Archive', provider: 's3', endpoint: 'https://minio.example:9000', region: 'us-east-1',
  bucket: 'archive', addressing: 'path', credential_source: 'static', access_key_id: 'AKIAEXAMPLE',
  account: null, has_secret: true, has_session_token: false, checksums: true, enabled: true,
  created_at: '2026-09-27T00:00:00Z', updated_at: '2026-09-27T00:00:00Z', ambient_custom_endpoint: false
}

const calls = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  delete: vi.fn(),
  confirm: vi.fn(async () => true)
}))

vi.mock('@/api/client', () => ({
  api: { GET: calls.get, POST: calls.post, PUT: calls.put, DELETE: calls.delete },
  responseError: () => 'failed',
  resultMessage: () => 'Object storage profile deleted'
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: calls.confirm() }) }) })
}))

/** The shared field stub with its description, which is where the "stored" state is said. */
const fieldWithDescription = {
  props: ['label', 'description'],
  template: '<div><label>{{ label }}<slot /></label><p v-if="description">{{ description }}</p></div>'
}

function mount() {
  return mountComponent(SettingsObjectStorageCard, {
    messages: { remote, common, server },
    stubs: { UFormField: fieldWithDescription }
  })
}

async function mounted() {
  const view = mount()
  await waitFor(() => expect(screen.getByText('Archive')).toBeTruthy())
  return view
}

function form(container: Element): HTMLFormElement {
  const element = container.querySelector('form')
  if (!element) throw new Error('no form')
  return element
}

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset()
  calls.get.mockResolvedValue({ data: [PROFILE] })
  calls.confirm.mockResolvedValue(true)
})

describe('SettingsObjectStorageCard form', () => {
  it('puts the provider first, the credential source next, and key fields only for stored keys', async () => {
    const { container } = await mounted()
    const fields = form(container).querySelectorAll('input, select')
    expect(fields[0]?.getAttribute('data-testid')).toBe('object-storage-provider')
    expect(fields[1]?.getAttribute('data-testid')).toBe('object-storage-source')
    expect(screen.getByLabelText(en.access_key_id)).toBeTruthy()

    await fireEvent.update(screen.getByTestId('object-storage-source'), 'ambient')

    expect(screen.queryByLabelText(en.access_key_id)).toBeNull()
    expect(screen.queryByLabelText(en.secret_access_key)).toBeNull()
    expect(screen.getByText(en.source_hints.ambient)).toBeTruthy()
  })

  it('asks Azure for its account and container, and offers a signature instead of S3 keys', async () => {
    await mounted()
    await fireEvent.update(screen.getByTestId('object-storage-provider'), 'azure')

    expect(screen.getByLabelText(en.account)).toBeTruthy()
    expect(screen.getByLabelText(en.container)).toBeTruthy()
    expect(screen.getByLabelText(en.account_key)).toBeTruthy()
    for (const s3Only of [en.region, en.addressing, en.access_key_id, en.session_token, en.bucket]) {
      expect(screen.queryByLabelText(s3Only)).toBeNull()
    }
    expect(screen.queryByRole('switch', { name: en.checksums })).toBeNull()

    await fireEvent.update(screen.getByTestId('object-storage-source'), 'shared_access_signature')
    expect(screen.getByLabelText(en.sas)).toBeTruthy()
    expect(screen.getByText(en.azure_hints.shared_access_signature)).toBeTruthy()
  })

  it('drops a signature source the next provider cannot use and asks Google for its key file', async () => {
    await mounted()
    await fireEvent.update(screen.getByTestId('object-storage-provider'), 'azure')
    await fireEvent.update(screen.getByTestId('object-storage-source'), 'shared_access_signature')
    await fireEvent.update(screen.getByTestId('object-storage-provider'), 'gcs')

    const source = screen.getByTestId('object-storage-source') as HTMLSelectElement
    expect(source.value).toBe('static')
    expect([...source.options].map(option => option.value)).toEqual(['static', 'ambient', 'anonymous'])
    expect(screen.getByLabelText(en.service_account_key)).toBeTruthy()
    expect(screen.queryByLabelText(en.account)).toBeNull()
    expect(screen.getByText(en.gcs_hints.static)).toBeTruthy()
  })

  it('sends an Azure profile with its account and signature, and nothing of S3', async () => {
    calls.post.mockResolvedValue({ data: { ...PROFILE, id: 'p2', provider: 'azure' } })
    const { container } = await mounted()
    await fireEvent.update(screen.getByTestId('object-storage-provider'), 'azure')
    await fireEvent.update(screen.getByTestId('object-storage-source'), 'shared_access_signature')
    await fireEvent.update(screen.getByLabelText(en.name), 'Blob')
    await fireEvent.update(screen.getByLabelText(en.account), 'mediaarchive')
    const submit = form(container).querySelector('button[type="submit"]') as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    await fireEvent.update(screen.getByLabelText(en.sas), 'sv=2024-11-04&sig=abc')
    expect(submit.disabled).toBe(false)
    await fireEvent.submit(form(container))

    await waitFor(() => expect(calls.post).toHaveBeenCalled())
    const [, init] = calls.post.mock.calls[0] as [string, { body: Record<string, unknown> }]
    expect(init.body).toMatchObject({
      provider: 'azure',
      account: 'mediaarchive',
      credential_source: 'shared_access_signature',
      secret_access_key: 'sv=2024-11-04&sig=abc',
      region: null,
      access_key_id: null,
      session_token: null,
      checksums: false
    })
    expect('addressing' in init.body).toBe(false)
  })

  it('ends with the primary action first and an icon-only cancel while editing', async () => {
    const { container } = await mounted()
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))

    const buttons = [...form(container).querySelectorAll('button:not([role="switch"])')]
    expect(buttons.map(button => button.getAttribute('type'))).toEqual(['submit', 'button'])
    expect(buttons[1]?.getAttribute('aria-label')).toBe(common.actions.cancel_edit)
    expect(buttons[1]?.textContent?.trim()).toBe('')
  })

  it('leaves a stored secret out of the form and out of the saved request', async () => {
    calls.put.mockResolvedValue({ data: { ...PROFILE, name: 'Renamed' } })
    await mounted()
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))

    const secret = screen.getByLabelText(en.secret_access_key) as HTMLInputElement
    expect(secret.value).toBe('')
    expect(screen.getByText(en.secret_keep)).toBeTruthy()

    await fireEvent.update(screen.getByLabelText(en.name), 'Renamed')
    await fireEvent.submit(secret.closest('form') as HTMLFormElement)

    await waitFor(() => expect(calls.put).toHaveBeenCalled())
    const [, init] = calls.put.mock.calls[0] as [string, { body: Record<string, unknown> }]
    expect(init.body.name).toBe('Renamed')
    expect(init.body.secret_access_key).toBeNull()
    expect(init.body.access_key_id).toBe('AKIAEXAMPLE')
    await waitFor(() => expect(screen.getByTestId('object-storage-message').textContent).toContain(en.saved))
  })

  it('says a changed endpoint drops the stored secret and asks for it again (RD-1190-20)', async () => {
    const { container } = await mounted()
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    const submit = form(container).querySelector('button[type="submit"]') as HTMLButtonElement
    expect(submit.disabled).toBe(false)

    await fireEvent.update(screen.getByLabelText(en.endpoint), 'https://collector.example')

    expect(screen.getByText(en.secret_host_changed)).toBeTruthy()
    expect(screen.queryByText(en.secret_keep)).toBeNull()
    expect(submit.disabled).toBe(true)
    await fireEvent.update(screen.getByLabelText(en.secret_access_key), 'typed again')
    expect(submit.disabled).toBe(false)
  })

  it('asks for the explicit yes before machine credentials go to an endpoint (RD-1190-20)', async () => {
    calls.post.mockResolvedValue({ data: { ...PROFILE, id: 'p3', credential_source: 'ambient' } })
    const { container } = await mounted()
    const submit = form(container).querySelector('button[type="submit"]') as HTMLButtonElement
    await fireEvent.update(screen.getByTestId('object-storage-source'), 'ambient')
    await fireEvent.update(screen.getByLabelText(en.name), 'Machine')
    expect(screen.queryByRole('switch', { name: en.ambient_custom_endpoint })).toBeNull()
    expect(submit.disabled).toBe(false)

    await fireEvent.update(screen.getByLabelText(en.endpoint), 'https://minio.example')
    expect(submit.disabled).toBe(true)
    await fireEvent.click(screen.getByRole('switch', { name: en.ambient_custom_endpoint }))
    expect(submit.disabled).toBe(false)
    await fireEvent.submit(form(container))

    await waitFor(() => expect(calls.post).toHaveBeenCalled())
    const [, init] = calls.post.mock.calls[0] as [string, { body: Record<string, unknown> }]
    expect(init.body).toMatchObject({ credential_source: 'ambient', endpoint: 'https://minio.example', ambient_custom_endpoint: true })
  })

  it('does not offer a new static profile without both keys', async () => {
    const { container } = await mounted()
    const submit = form(container).querySelector('button[type="submit"]') as HTMLButtonElement
    await fireEvent.update(screen.getByLabelText(en.name), 'New')
    expect(submit.disabled).toBe(true)
    await fireEvent.update(screen.getByLabelText(en.access_key_id), 'AKIA')
    await fireEvent.update(screen.getByLabelText(en.secret_access_key), 'secret')
    expect(submit.disabled).toBe(false)
  })
})

describe('SettingsObjectStorageCard rows', () => {
  it('shows a failed test as the translated code with its parameters', async () => {
    calls.post.mockResolvedValue({
      data: { reachable: true, authenticated: false, code: 'object_storage.access_denied', params: { bucket: 'archive' } }
    })
    await mounted()
    await fireEvent.click(screen.getByRole('button', { name: common.actions.test }))

    const alert = await waitFor(() => screen.getByTestId('object-storage-error'))
    expect(alert.textContent).toContain('Access to the bucket archive was denied')
  })

  it('shows a passed test as a success above the form', async () => {
    calls.post.mockResolvedValue({ data: { reachable: true, authenticated: true, code: null, params: {} } })
    await mounted()
    await fireEvent.click(screen.getByRole('button', { name: common.actions.test }))

    const alert = await waitFor(() => screen.getByTestId('object-storage-message'))
    expect(alert.textContent).toContain('Archive:')
  })

  it('asks before deleting, and deletes nothing when the answer is no', async () => {
    calls.confirm.mockResolvedValue(false)
    await mounted()
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }))

    await waitFor(() => expect(calls.confirm).toHaveBeenCalled())
    expect(calls.delete).not.toHaveBeenCalled()
    expect(screen.getByText('Archive')).toBeTruthy()
  })

  it('drops the row and leaves edit mode when the edited profile is deleted', async () => {
    calls.delete.mockResolvedValue({ data: { message: 'deleted' } })
    await mounted()
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    expect(screen.getByText(en.form_edit)).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }))

    await waitFor(() => expect(screen.queryAllByTestId('object-storage-row')).toHaveLength(0))
    expect(screen.getByText(en.form_new)).toBeTruthy()
    expect(screen.getByText(en.empty)).toBeTruthy()
  })
})
