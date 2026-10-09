/**
 * Clearing the remote jobs list, here only or at the provider too (RD-1200-01).
 *
 * Mounted inside the card, because the menu acts on what the card's filters show: the number
 * in the question, the providers it names and the filter the request carries all come from the
 * same narrowed list. A job still running is left out and the question says how many; a
 * question answered with no sends nothing at all.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Account } from '@/api/types'
import en from '@/locales/en/remote_jobs.json'
import server from '@/locales/en/server.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import { clearBody, clearSelection, filterJobs, jobProviders } from './remoteJobsFilter'
import SettingsRemoteJobsCard from './SettingsRemoteJobsCard.vue'

const REALDEBRID = { id: 'a1', provider: 'realdebrid', label: 'RD account', enabled: true } as unknown as Account
const TORBOX = { id: 'a2', provider: 'torbox', label: 'TB account', enabled: true } as unknown as Account

const BASE = {
  plugin_id: 'p1',
  source_kind: 'magnet',
  submit_attempts: 1,
  adoption_checked: false,
  created_at: '2026-10-08T10:00:00Z',
  updated_at: '2026-10-08T10:05:00Z'
}
const READY = { ...BASE, id: 'j1', account_id: 'a1', content_key: 'k1', remote_id: 'RD-1', state: 'ready' }
const FAILED = { ...BASE, id: 'j2', account_id: 'a2', content_key: 'k2', remote_id: 'TB-1', state: 'failed' }
const WORKING = { ...BASE, id: 'j3', account_id: 'a1', content_key: 'k3', remote_id: 'RD-2', state: 'working' }

const state = vi.hoisted(() => ({
  jobs: [] as unknown[],
  confirm: true,
  asked: [] as { title: string, description: string }[],
  posts: [] as { path: string, init: { body: unknown } }[],
  answer: null as unknown
}))

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/remote-jobs') return { data: state.jobs }
      if (path === '/api/v1/remote-jobs/providers') return { data: ['realdebrid', 'torbox'] }
      if (path === '/api/v1/providers') {
        return { data: [{ slug: 'realdebrid', display_name: 'Real-Debrid', credentials: 'secret', kind: 'hoster' }, { slug: 'torbox', display_name: 'TorBox', credentials: 'secret', kind: 'hoster' }] }
      }
      return { data: [] }
    }),
    POST: vi.fn(async (path: string, init: { body: unknown }) => {
      state.posts.push({ path, init })
      return state.answer
    }),
    DELETE: vi.fn(async () => ({ data: {} }))
  },
  responseError: () => 'failed'
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
/** The route the filter reads and writes (`useRemoteJobsFilter`, RD-1200-01), reactive like the real one. */
const router = vi.hoisted(() => ({
  route: null as unknown as { path: string, hash: string, query: Record<string, string | undefined> },
  replace: null as unknown as ReturnType<typeof vi.fn>
}))
vi.mock('vue-router', async () => {
  const { reactive } = await import('vue')
  router.route = reactive({ path: '/remote-jobs', hash: '', query: {} })
  router.replace = vi.fn(async ({ query }: { query: Record<string, string | undefined> }) => {
    router.route.query = { ...query }
  })
  return { useRoute: () => router.route, useRouter: () => ({ replace: router.replace }) }
})
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({
    create: () => ({
      open: (options: { title: string, description: string }) => {
        state.asked.push(options)
        return { result: Promise.resolve(state.confirm) }
      }
    })
  })
}))

function mount(jobs: unknown[], query: Record<string, string> = {}) {
  router.route.query = { ...query }
  router.replace.mockClear()
  state.jobs = jobs
  state.confirm = true
  state.asked.length = 0
  state.posts.length = 0
  state.answer = { data: { removed: 0, failed: 0, results: [], skipped: [] } }
  return mountComponent(SettingsRemoteJobsCard, {
    messages: { remote_jobs: en, server },
    props: { accounts: [REALDEBRID, TORBOX] }
  })
}

const localEntry = (count: number) => en.clear.local.action.replace('{count}', String(count))
const providerEntry = (count: number) => en.clear.provider.action.replace('{count}', String(count))

describe('remoteJobsFilter', () => {
  const jobs = [READY, FAILED, WORKING] as never[]

  it('narrows by provider and state, and leaves a running job out of the clear', () => {
    expect(filterJobs(jobs, [REALDEBRID, TORBOX], { provider: 'realdebrid', state: 'all' }).map(job => (job as { id: string }).id)).toEqual(['j1', 'j3'])
    const selection = clearSelection(filterJobs(jobs, [REALDEBRID, TORBOX], { provider: 'all', state: 'all' }), [REALDEBRID, TORBOX])
    expect(selection.targets.length).toBe(2)
    expect(selection.running.length).toBe(1)
    expect(selection.providers).toEqual(['realdebrid', 'torbox'])
  })

  it('knows no provider for a job whose account is gone', () => {
    expect(jobProviders([{ ...READY, account_id: 'gone' }] as never[], [REALDEBRID])).toEqual([])
  })

  it('sends the filter, not ids, and the confirmation', () => {
    expect(clearBody({ provider: 'all', state: 'all' }, false)).toEqual({ confirmed: true, at_provider: false, states: [] })
    expect(clearBody({ provider: 'torbox', state: 'failed' }, true)).toEqual({ confirmed: true, at_provider: true, provider: 'torbox', states: ['failed'] })
  })
})

describe('RemoteJobsClearMenu', () => {
  it('names the count, the providers and the jobs left running, and clears here only', async () => {
    mount([READY, FAILED, WORKING])
    const entry = await waitFor(() => screen.getByRole('button', { name: localEntry(2) }))
    state.answer = { data: { removed: 2, failed: 0, results: [], skipped: [{ id: 'j3', removed: false }] } }
    await fireEvent.click(entry)
    await waitFor(() => expect(state.posts.length).toBe(1))
    const question = state.asked[0]?.description ?? ''
    expect(question).toContain('2 jobs at Real-Debrid, TorBox')
    expect(question).toContain('1 job is still running and stays.')
    expect(state.posts[0]).toEqual({
      path: '/api/v1/remote-jobs/clear',
      init: { body: { confirmed: true, at_provider: false, states: [] } }
    })
  })

  it('sends nothing when the question is answered with no', async () => {
    mount([READY, FAILED])
    const entry = await waitFor(() => screen.getByRole('button', { name: providerEntry(2) }))
    state.confirm = false
    await fireEvent.click(entry)
    await waitFor(() => expect(state.asked.length).toBe(1))
    expect(state.asked[0]?.title).toBe(en.clear.provider.confirm_title)
    expect(state.posts).toEqual([])
  })

  it('acts on the filtered list only, and says which jobs a provider kept', async () => {
    const view = mount([READY, FAILED, WORKING])
    await waitFor(() => screen.getByRole('button', { name: localEntry(2) }))
    await fireEvent.update(screen.getByLabelText(en.filter.provider), 'torbox')
    const entry = await waitFor(() => screen.getByRole('button', { name: providerEntry(1) }))
    state.answer = {
      data: { removed: 0, failed: 1, results: [{ id: 'j2', provider: 'torbox', removed: false, code: 'torbox.offline', message: 'TorBox did not answer' }], skipped: [] }
    }
    await fireEvent.click(entry)
    await waitFor(() => expect(state.posts.length).toBe(1))
    expect(state.asked[0]?.description).toContain('at TorBox')
    expect(state.asked[0]?.description).not.toContain('still running')
    expect(state.posts[0]?.init.body).toEqual({ confirmed: true, at_provider: true, provider: 'torbox', states: [] })
    await waitFor(() => expect((view.emitted('error') as string[][] | undefined)?.[0]?.[0]).toContain('TorBox did not answer'))
  })

  it('keeps the filters in the address, so a reload opens the list as narrowed as it was', async () => {
    const before = mount([READY, FAILED, WORKING])
    await waitFor(() => screen.getByRole('button', { name: localEntry(2) }))
    expect(router.replace).not.toHaveBeenCalled()
    await fireEvent.update(screen.getByLabelText(en.filter.provider), 'torbox')
    await fireEvent.update(screen.getByLabelText(en.filter.state), 'failed')
    await waitFor(() => expect(router.route.query).toEqual({ provider: 'torbox', state: 'failed' }))
    const reloaded = { ...router.route.query } as Record<string, string>
    before.unmount()

    // A reload is a fresh mount on the same address.
    mount([READY, FAILED, WORKING], reloaded)
    await waitFor(() => screen.getByRole('button', { name: localEntry(1) }))
    expect((screen.getByLabelText(en.filter.provider) as HTMLSelectElement).value).toBe('torbox')
    expect((screen.getByLabelText(en.filter.state) as HTMLSelectElement).value).toBe('failed')
    expect(router.replace).not.toHaveBeenCalled()

    // Back to everything: the address is plain again.
    await fireEvent.update(screen.getByLabelText(en.filter.provider), 'all')
    await fireEvent.update(screen.getByLabelText(en.filter.state), 'all')
    await waitFor(() => expect(router.route.query).toEqual({}))
  })

  it('offers nothing to clear while every shown job is still running', async () => {
    mount([WORKING])
    const entry = await waitFor(() => screen.getByRole('button', { name: localEntry(0) }))
    expect((entry as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: providerEntry(0) }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('says when the filter hides every job, with the way back', async () => {
    mount([READY])
    await waitFor(() => screen.getByRole('button', { name: localEntry(1) }))
    await fireEvent.update(screen.getByLabelText(en.filter.state), 'failed')
    await waitFor(() => expect(screen.getByText(en.filter.none)).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: en.filter.reset }))
    await waitFor(() => expect(screen.queryByText(en.filter.none)).toBeNull())
  })

  it('has no accessibility violations with the filters and the menu drawn', async () => {
    const { container } = mount([READY, FAILED, WORKING])
    await waitFor(() => screen.getByRole('button', { name: localEntry(2) }))
    expect(await axeViolations(container)).toBe('')
  })
})
