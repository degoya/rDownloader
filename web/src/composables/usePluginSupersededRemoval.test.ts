/**
 * Removing every superseded version at once (RD-1140-04): "something to delete all superseded
 * versions of plugins, so that I do not have to delete every version one by one". One
 * confirmation that names the number, one request — for every plugin or for one — and a toast
 * that says what went and, with its reason, what stayed.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

const add = vi.fn()
const DELETE = vi.fn()
const confirmed = vi.fn<(options: Record<string, unknown>) => Promise<boolean>>()

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string, named?: Record<string, unknown>) => named ? `${key} ${JSON.stringify(named)}` : key
  })
}))
vi.mock('@/api/client', () => ({
  api: { DELETE },
  responseError: () => 'The service did not answer'
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))
vi.mock('@/i18n/server', () => ({
  translateServerMessage: (message: { code: string }) => `[${message.code}]`
}))

const { usePluginSupersededRemoval } = await import('./usePluginSupersededRemoval')

const PLUGIN = 'com.example.resolver.2'

function setup() {
  const message = ref<string | null>('earlier answer')
  const error = ref<string | null>(null)
  const done = vi.fn(async () => {})
  const { removeSuperseded } = usePluginSupersededRemoval({ message, error, done })
  return { message, error, done, removeSuperseded }
}

describe('removing every superseded plugin version', () => {
  beforeEach(() => {
    add.mockReset()
    DELETE.mockReset()
    confirmed.mockReset()
    confirmed.mockResolvedValue(true)
  })

  it('asks once with the number and removes every plugin\'s leftovers', async () => {
    DELETE.mockResolvedValue({
      data: {
        code: 'plugin.superseded_removed',
        message: '',
        removed: [{ plugin_id: PLUGIN, name: 'Resolver', version: '0.9.0' }, { plugin_id: PLUGIN, name: 'Resolver', version: '0.8.0' }],
        kept: []
      }
    })
    const { message, done, removeSuperseded } = setup()

    await removeSuperseded({ count: 2 })

    expect(confirmed.mock.calls[0]?.[0]).toMatchObject({
      title: 'plugins.remove.superseded_all_title',
      description: 'plugins.remove.superseded_all_description {"count":2}',
      destructive: true,
      confirmIcon: 'i-lucide-trash-2'
    })
    expect(DELETE.mock.calls).toEqual([['/api/v1/plugins/superseded']])
    expect(message.value).toBeNull()
    expect(add).toHaveBeenCalledWith(expect.objectContaining({
      title: 'plugins.remove.superseded_done {"removed":2,"kept":0}',
      color: 'success'
    }))
    expect(add.mock.calls[0]?.[0]).not.toHaveProperty('description')
    expect(done).toHaveBeenCalled()
  })

  it('removes one plugin\'s leftovers and names what stayed with its reason', async () => {
    DELETE.mockResolvedValue({
      data: {
        code: 'plugin.superseded_partly_removed',
        message: '',
        removed: [{ plugin_id: PLUGIN, name: 'Resolver', version: '0.8.0' }],
        kept: [{ plugin_id: PLUGIN, name: 'Resolver', version: '0.9.0', reason: { code: 'plugin.version_in_use', message: '' } }]
      }
    })
    const { removeSuperseded } = setup()

    await removeSuperseded({ count: 2, plugin: { id: PLUGIN, name: 'Resolver' } })

    expect(confirmed.mock.calls[0]?.[0]).toMatchObject({
      description: 'plugins.remove.superseded_plugin_description {"count":2,"name":"Resolver"}'
    })
    expect(DELETE.mock.calls).toEqual([[
      '/api/v1/plugins/{id}/superseded',
      { params: { path: { id: PLUGIN } } }
    ]])
    expect(add).toHaveBeenCalledWith({
      title: 'plugins.remove.superseded_done {"removed":1,"kept":1}',
      description: 'Resolver v0.9.0: [plugin.version_in_use]',
      color: 'warning',
      icon: 'i-lucide-circle-alert'
    })
  })

  it('does nothing when the confirmation is declined', async () => {
    confirmed.mockResolvedValue(false)
    const { done, removeSuperseded } = setup()

    await removeSuperseded({ count: 3 })

    expect(DELETE).not.toHaveBeenCalled()
    expect(add).not.toHaveBeenCalled()
    expect(done).not.toHaveBeenCalled()
  })

  it('shows a refused request in the tab\'s error line, not as a toast', async () => {
    DELETE.mockResolvedValue({ error: { code: 'auth.scope_insufficient' } })
    const { error, done, removeSuperseded } = setup()

    await removeSuperseded({ count: 1 })

    expect(error.value).toBe('The service did not answer')
    expect(add).not.toHaveBeenCalled()
    expect(done).toHaveBeenCalled()
  })
})
