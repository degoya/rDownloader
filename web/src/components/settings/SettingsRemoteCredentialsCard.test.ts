/**
 * The SSH host-key prompt, which is the one thing in this card a reader must not click past.
 *
 * An unknown key is the ordinary first answer from a new server. A *changed* key is the answer a
 * machine-in-the-middle also produces, and the two must not look alike: the changed case has to
 * name the fingerprint that was stored beside the one now offered, so the reader compares them
 * rather than confirming on trust. Neither is ever trusted by testing alone — confirming is a
 * second, separate press.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import en from '@/locales/en/remote.json'
import server from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'

import SettingsRemoteCredentialsCard from './SettingsRemoteCredentialsCard.vue'

const CREDENTIAL = {
  id: 'c1', name: 'Backup box', protocol: 'sftp', host: 'files.invalid', port: 22,
  auth_mode: 'password', username: 'rd', has_password: true, has_private_key: false,
  passive: true, verify_tls: true
}

const testResult = vi.hoisted(() => ({ value: {} as Record<string, unknown> }))
const trusted = vi.hoisted(() => vi.fn(async () => ({ data: { id: 'k1' } })))

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async (path: string) => ({ data: path.includes('ssh-hosts') ? [] : [CREDENTIAL] })),
    POST: vi.fn(async (path: string) => (path.includes('/test')
      ? { data: testResult.value }
      : trusted())),
    PUT: vi.fn(async () => ({ data: CREDENTIAL })),
    DELETE: vi.fn(async () => ({ data: { message: 'gone' } }))
  },
  responseError: () => 'failed',
  resultMessage: () => 'done'
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }) })
}))

function mount() {
  return mountComponent(SettingsRemoteCredentialsCard, {
    messages: { remote: en, server },
    props: { settings: {} as Settings }
  })
}

async function runTest() {
  mount()
  const button = await waitFor(() => screen.getByRole('button', { name: en.credentials.test }))
  await fireEvent.click(button)
}

describe('SettingsRemoteCredentialsCard host keys', () => {
  it('asks about an unknown key instead of reporting it as a plain failure', async () => {
    testResult.value = {
      authenticated: false,
      code: 'sftp.host_key_unknown',
      params: { host: 'files.invalid', port: '22', algorithm: 'ssh-ed25519', fingerprint: 'SHA256:aaa' }
    }
    await runTest()
    await waitFor(() => expect(screen.getByText(en.host_keys.unknown_title)).toBeTruthy())
    expect(screen.getByText('SHA256:aaa')).toBeTruthy()
    expect(screen.queryByText(en.host_keys.changed_title)).toBeNull()
  })

  /** The dangerous case: both fingerprints are on screen, so the reader can compare them. */
  it('names the stored fingerprint beside the offered one when a key has changed', async () => {
    testResult.value = {
      authenticated: false,
      code: 'sftp.host_key_changed',
      params: {
        host: 'files.invalid', port: '22', algorithm: 'ssh-ed25519',
        fingerprint: 'SHA256:new', stored_fingerprint: 'SHA256:old'
      }
    }
    await runTest()
    await waitFor(() => expect(screen.getByText(en.host_keys.changed_title)).toBeTruthy())
    expect(screen.getByText('SHA256:new')).toBeTruthy()
    expect(screen.getByText('SHA256:old')).toBeTruthy()
  })

  it('trusts nothing until the key is confirmed, and nothing at all if it is rejected', async () => {
    testResult.value = {
      authenticated: false,
      code: 'sftp.host_key_unknown',
      params: { host: 'files.invalid', port: '22', algorithm: 'ssh-ed25519', fingerprint: 'SHA256:aaa' }
    }
    await runTest()
    await waitFor(() => expect(screen.getByText(en.host_keys.unknown_title)).toBeTruthy())
    expect(trusted).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByText(en.host_keys.reject))
    await waitFor(() => expect(screen.queryByText(en.host_keys.unknown_title)).toBeNull())
    expect(trusted).not.toHaveBeenCalled()
  })

  it('leaves an ordinary failure as a message rather than a key prompt', async () => {
    testResult.value = {
      authenticated: false,
      code: 'sftp.auth_failed',
      params: {}
    }
    await runTest()
    await waitFor(() => expect(screen.queryByText(en.host_keys.unknown_title)).toBeNull())
    expect(screen.queryByText(en.host_keys.changed_title)).toBeNull()
  })
})
