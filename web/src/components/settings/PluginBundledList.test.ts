/**
 * The plugin manager's "Available services" (RD-160-05): what the release ships and is not
 * installed is listed, installed with one click, and never installed without one.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import plugins from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

const listBundled = vi.fn()
const installBundled = vi.fn()
vi.mock('@/api/bundledPlugins', () => ({
  listBundled: (...args: unknown[]) => listBundled(...args),
  installBundled: (...args: unknown[]) => installBundled(...args)
}))

const { default: PluginBundledList } = await import('./PluginBundledList.vue')

function service(key: string, name: string, state: string) {
  return {
    key,
    name,
    description: `${name} description`,
    category: 'hoster',
    needs_account: true,
    provider: key,
    state,
    plugins: [{ id: `id-${key}`, name, plugin_type: 'resolver', version: '1.0.0', installed_version: null }]
  }
}

describe('PluginBundledList', () => {
  beforeEach(() => {
    listBundled.mockReset().mockResolvedValue({
      ok: true,
      data: { services: [service('rapidgator', 'Rapidgator', 'available'), service('mediafire', 'MediaFire', 'installed')] }
    })
    installBundled.mockReset().mockResolvedValue({
      ok: true,
      data: { code: 'plugin.bundled_installed', message: '', installed: [{ id: 'id-rapidgator' }], failed: [] }
    })
  })

  it('lists only what is not installed, and installs it with one click', async () => {
    const { emitted } = mountComponent(PluginBundledList, { messages: { plugins } })

    const row = await screen.findByTestId('bundled-service-rapidgator')
    expect(screen.queryByTestId('bundled-service-mediafire')).toBeNull()
    expect(installBundled).not.toHaveBeenCalled()

    await fireEvent.click(within(row).getByRole('button', { name: plugins.bundled.install }))

    await waitFor(() => expect(emitted().installed).toHaveLength(1))
    expect(installBundled).toHaveBeenCalledWith(['rapidgator'])
    expect(emitted().installed?.[0]).toEqual(['1 plugin installed; it runs from the next start.'])
  })

  it('says so when everything in the bundle is installed', async () => {
    listBundled.mockResolvedValue({ ok: true, data: { services: [service('mediafire', 'MediaFire', 'installed')] } })
    mountComponent(PluginBundledList, { messages: { plugins } })

    expect(await screen.findByText(plugins.bundled.all_installed)).toBeTruthy()
  })
})
