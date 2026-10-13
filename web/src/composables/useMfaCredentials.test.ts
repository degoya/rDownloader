/**
 * Removing a second factor asks first, sends the password asked for again, and clears it only
 * once the service took it (RD-1280-11).
 */
import { ref } from 'vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const DELETE = vi.fn()
vi.mock('@/api/client', () => ({
  api: { DELETE: (...args: unknown[]) => DELETE(...args) },
  responseError: () => 'wrong password'
}))

const { useMfaCredentials } = await import('./useMfaCredentials')

const credential = { id: 'cred-1', kind: 'totp', label: 'Phone' } as never
const question = { title: 'Remove?', description: 'Phone', destructive: true }

function card(answer: boolean) {
  const confirm = vi.fn(() => Promise.resolve(answer))
  const password = ref('secret')
  const error = ref<string | null>(null)
  const reload = vi.fn(() => Promise.resolve())
  return { confirm, password, error, reload }
}

describe('removing a second factor', () => {
  beforeEach(() => DELETE.mockReset())

  it('does nothing when the question is declined', async () => {
    const state = card(false)
    await useMfaCredentials(state).remove(credential, question)
    expect(state.confirm).toHaveBeenCalledWith(question)
    expect(DELETE).not.toHaveBeenCalled()
    expect(state.password.value).toBe('secret')
  })

  it('sends the password, clears it and fetches the list anew', async () => {
    DELETE.mockResolvedValue({ data: {} })
    const state = card(true)
    await useMfaCredentials(state).remove(credential, question)
    expect(DELETE).toHaveBeenCalledWith('/api/v1/mfa/credentials/{id}', { params: { path: { id: 'cred-1' } }, body: { password: 'secret' } })
    expect(state.password.value).toBe('')
    expect(state.reload).toHaveBeenCalledOnce()
  })

  it('keeps the password and says why when the service refuses', async () => {
    DELETE.mockResolvedValue({ error: { code: 'auth.password_wrong' } })
    const state = card(true)
    await useMfaCredentials(state).remove(credential, question)
    expect(state.error.value).toBe('wrong password')
    expect(state.password.value).toBe('secret')
    expect(state.reload).not.toHaveBeenCalled()
  })
})
