/**
 * The version controls of one plugin (RD-140-02), as an action matrix: which control is offered
 * in which state, and which request each one sends. Every action takes effect at the next start,
 * so the panel's other job is to keep "runs now" and "runs from the next start" apart.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import pluginsCatalogue from '@/locales/en/plugins.json'
import { mountComponent } from '@/test/mount'

const post = vi.fn(async () => ({ data: { code: 'ok', message: 'ok' } }))
const put = vi.fn(async () => ({ data: { code: 'ok', message: 'ok' } }))
const remove = vi.fn(async () => ({ data: { code: 'ok', message: 'ok' } }))
vi.mock('@/api/client', () => ({
  api: {
    POST: (...args: unknown[]) => post(...(args as [])),
    PUT: (...args: unknown[]) => put(...(args as [])),
    DELETE: (...args: unknown[]) => remove(...(args as []))
  },
  responseError: () => 'refused',
  resultMessage: () => 'saved'
}))

const { default: PluginVersionPanel } = await import('./PluginVersionPanel.vue')

const ID = '019d0000-0000-7000-8000-000000001402'

function lifecycle(overrides: Record<string, unknown> = {}) {
  return {
    plugin_id: ID,
    active_version: '2.0.0',
    running_version: '2.0.0',
    staged_version: null,
    previous_version: null,
    update_policy: 'manual',
    restart_required: false,
    ...overrides
  }
}

function mount(entry: Record<string, unknown>, versions = ['2.0.0', '1.0.0']) {
  return mountComponent(PluginVersionPanel, {
    messages: { plugins: pluginsCatalogue },
    props: { lifecycle: entry, versions }
  })
}

const path = { params: { path: { id: ID } } }

beforeEach(() => {
  post.mockClear()
  put.mockClear()
  remove.mockClear()
})

describe('PluginVersionPanel', () => {
  it('names the running version and offers neither a test nor a rollback it cannot do', () => {
    mount(lifecycle(), ['2.0.0'])

    expect(screen.getByText('Running: v2.0.0')).toBeTruthy()
    expect(screen.queryByText('Restart to apply')).toBeNull()
    expect(screen.queryByRole('button', { name: /Roll back/ })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Test' })).toBeNull()
  })

  it('keeps the running version and the one of the next start apart until a restart', () => {
    mount(lifecycle({ active_version: '1.0.0', previous_version: '2.0.0', restart_required: true }))

    expect(screen.getByText('Running: v2.0.0')).toBeTruthy()
    expect(screen.getByText('From the next start: v1.0.0')).toBeTruthy()
    expect(screen.getByText('Restart to apply')).toBeTruthy()
  })

  it('puts a picked version under test, or activates it', async () => {
    const view = mount(lifecycle())
    await fireEvent.update(screen.getByLabelText('Another installed version'), '1.0.0')

    await fireEvent.click(screen.getByRole('button', { name: 'Test' }))
    expect(post).toHaveBeenCalledWith('/api/v1/plugins/{id}/lifecycle/stage', { ...path, body: { version: '1.0.0' } })
    expect(view.emitted('done')).toEqual([[{ message: 'saved', error: null }]])

    await fireEvent.update(screen.getByLabelText('Another installed version'), '1.0.0')
    await fireEvent.click(screen.getByRole('button', { name: 'Activate' }))
    expect(post).toHaveBeenLastCalledWith('/api/v1/plugins/{id}/lifecycle/activate', { ...path, body: { version: '1.0.0' } })
  })

  it('activates or discards the version under test', async () => {
    mount(lifecycle({ staged_version: '3.0.0' }), ['3.0.0', '2.0.0'])
    expect(screen.getByText('Under test: v3.0.0')).toBeTruthy()

    const [activateStaged] = screen.getAllByRole('button', { name: 'Activate' })
    await fireEvent.click(activateStaged!)
    expect(post).toHaveBeenCalledWith('/api/v1/plugins/{id}/lifecycle/activate', { ...path, body: { version: '3.0.0' } })

    await fireEvent.click(screen.getByRole('button', { name: 'Discard' }))
    expect(remove).toHaveBeenCalledWith('/api/v1/plugins/{id}/lifecycle/stage', path)
  })

  it('rolls back to the previous version in one request', async () => {
    mount(lifecycle({ previous_version: '1.0.0' }))

    await fireEvent.click(screen.getByRole('button', { name: 'Roll back to v1.0.0' }))
    expect(post).toHaveBeenCalledWith('/api/v1/plugins/{id}/lifecycle/rollback', path)
  })

  it('switches the update policy', async () => {
    mount(lifecycle())

    await fireEvent.click(screen.getByRole('switch', { name: 'Install updates automatically' }))
    expect(put).toHaveBeenCalledWith('/api/v1/plugins/{id}/lifecycle/policy', { ...path, body: { policy: 'automatic' } })
  })

  it('hands a refusal to the tab instead of a message', async () => {
    post.mockResolvedValueOnce({ error: { code: 'plugin.no_previous_version' } } as never)
    const view = mount(lifecycle({ previous_version: '1.0.0' }))

    await fireEvent.click(screen.getByRole('button', { name: 'Roll back to v1.0.0' }))
    expect(view.emitted('done')).toEqual([[{ message: null, error: 'refused' }]])
  })

  /** Notes come from a repository, so markup in them must reach the reader as text. */
  it('lists the release notes the indexes delivered as plain text, and no section without any', () => {
    const { unmount } = mountComponent(PluginVersionPanel, {
      messages: { plugins: pluginsCatalogue },
      props: {
        lifecycle: lifecycle(),
        versions: ['2.0.0'],
        releaseNotes: [
          { version: '2.1.0', notes: '<b>Faster</b> downloads', repository: 'rDownloader' },
          { version: '2.0.0', notes: 'First line\nSecond line', repository: 'rDownloader' }
        ]
      }
    })
    const section = document.querySelector('[data-release-notes]') as HTMLElement
    expect(section).toBeTruthy()
    expect(section.textContent).toContain('v2.1.0')
    expect(section.textContent).toContain('<b>Faster</b> downloads')
    expect(section.querySelector('b')).toBeNull()
    unmount()

    mount(lifecycle(), ['2.0.0'])
    expect(document.querySelector('[data-release-notes]')).toBeNull()
  })
})
