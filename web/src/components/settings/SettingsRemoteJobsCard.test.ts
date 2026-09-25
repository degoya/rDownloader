/**
 * The two ways a remote job ends, and the one thing that must never happen by accident.
 *
 * Deleting at the provider reaches into an account on a machine that is not this one and
 * cannot be undone, so it is worth a test of its own that a rejected confirmation sends
 * nothing at all — and that clearing a row out of the list never touches that endpoint. The
 * file selection is here for the same reason in the other direction: the question has to be
 * asked, and only what the job offered may be answered with.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Account } from '@/api/types'
import en from '@/locales/en/remote_jobs.json'
import server from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'

import SettingsRemoteJobsCard from './SettingsRemoteJobsCard.vue'

const ACCOUNT = { id: 'a1', provider: 'realdebrid', label: 'Real-Debrid', enabled: true } as unknown as Account
/** The second `remote-job` provider, which is what makes the filter below testable at all. */
const PREMIUMIZE = { id: 'a2', provider: 'premiumize', label: 'Premiumize', enabled: true } as unknown as Account
/** A service with no `remote-job` plugin. The owner's own case, reported from use. */
const DDOWNLOAD = { id: 'a3', provider: 'ddownload', label: 'DDownload', enabled: true } as unknown as Account

const WORKING = {
  id: 'j1',
  account_id: 'a1',
  plugin_id: 'p1',
  content_key: 'da39a3ee',
  remote_id: 'REMOTE01',
  state: 'working',
  source_kind: 'magnet',
  submit_attempts: 1,
  adoption_checked: false,
  progress_permille: 420,
  created_at: '2026-09-20T10:00:00Z',
  updated_at: '2026-09-20T10:05:00Z'
}

const ASKING = {
  ...WORKING,
  id: 'j2',
  remote_id: 'REMOTE02',
  state: 'awaiting_choice',
  progress_permille: null,
  entries: [
    { id: 1, path: 'Example/ep01.mkv', size: 10, selected: false },
    { id: 2, path: 'Example/ep02.mkv', size: null, selected: true }
  ]
}

const jobs = vi.hoisted(() => ({ value: [] as unknown[] }))
/** What `/api/v1/remote-jobs/providers` answers; `null` makes the request fail. */
const providers = vi.hoisted(() => ({ value: ['realdebrid', 'premiumize'] as string[] | null }))
/** When set, the providers request waits for this instead of answering at once. */
const providersGate = vi.hoisted(() => ({ value: null as Promise<void> | null }))
/**
 * What the job route answers, one entry per request, in order; empty means "started". A
 * function is awaited, so a test can hold one request open and look at the others.
 */
const submitAnswers = vi.hoisted(() => [] as (unknown | (() => Promise<unknown>))[])
const confirmResult = vi.hoisted(() => ({ value: true }))
const posts = vi.hoisted(() => [] as { path: string, init: unknown }[])
const deletes = vi.hoisted(() => [] as string[])

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/remote-jobs/providers') {
        if (providersGate.value) await providersGate.value
        return providers.value ? { data: providers.value } : { error: { code: 'x' } }
      }
      return { data: jobs.value }
    }),
    POST: vi.fn(async (path: string, init: unknown) => {
      posts.push({ path, init })
      if (path === '/api/v1/accounts/{id}/remote-jobs') {
        const next = submitAnswers.shift()
        if (typeof next === 'function') return await (next as () => Promise<unknown>)()
        if (next) return next
        return { data: { job: { ...WORKING, id: `j${posts.length + 2}`, source_kind: 'container' }, already_running: false } }
      }
      return { data: { ...ASKING, state: 'working' } }
    }),
    DELETE: vi.fn(async (path: string) => {
      deletes.push(path)
      return { data: { message: 'gone', code: 'remote_job.removed' } }
    })
  },
  responseError: (response: { error?: { code?: string } }) => response.error?.code ?? 'failed'
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(confirmResult.value) }) }) })
}))

function mount(accounts: Account[] = [ACCOUNT], installed: string[] | null = ['realdebrid', 'premiumize']) {
  posts.length = 0
  deletes.length = 0
  submitAnswers.length = 0
  providers.value = installed
  return mountComponent(SettingsRemoteJobsCard, {
    messages: { remote_jobs: en, server },
    props: { accounts }
  })
}

describe('SettingsRemoteJobsCard', () => {
  it('shows a running job with the stage it is in and the progress the provider measured', async () => {
    jobs.value = [WORKING]
    mount()
    await waitFor(() => expect(screen.getByText(en.states.working)).toBeTruthy())
    expect(screen.getByText('42%')).toBeTruthy()
    expect(screen.getByText('REMOTE01')).toBeTruthy()
  })

  it('asks a job that waits for a selection, and sends only what it offered', async () => {
    jobs.value = [ASKING]
    mount()
    const toggle = await waitFor(() => screen.getByRole('button', { name: en.choice.toggle }))
    await fireEvent.click(toggle)
    await waitFor(() => expect(screen.getByText(en.choice.title)).toBeTruthy())

    // Both offered entries are shown, and nothing is picked on the reader's behalf.
    await fireEvent.click(screen.getByLabelText('Example/ep02.mkv'))
    await fireEvent.click(screen.getByRole('button', { name: en.choice.action }))

    await waitFor(() => expect(posts.length).toBe(1))
    expect(posts[0]?.path).toBe('/api/v1/remote-jobs/{id}/choice')
    expect(posts[0]?.init).toMatchObject({ body: { entries: [2] } })
  })

  it('sends nothing to the provider when the deletion is not confirmed', async () => {
    jobs.value = [WORKING]
    confirmResult.value = false
    mount()
    const button = await waitFor(() => screen.getByRole('button', { name: en.discard.action }))
    await fireEvent.click(button)
    await waitFor(() => expect(screen.getByText(en.states.working)).toBeTruthy())
    expect(posts).toEqual([])
    confirmResult.value = true
  })

  it('carries the confirmation to the server when it was given', async () => {
    jobs.value = [WORKING]
    mount()
    const button = await waitFor(() => screen.getByRole('button', { name: en.discard.action }))
    await fireEvent.click(button)
    await waitFor(() => expect(posts.length).toBe(1))
    expect(posts[0]?.path).toBe('/api/v1/remote-jobs/{id}/discard')
    expect(posts[0]?.init).toMatchObject({ body: { confirmed: true } })
  })

  /**
   * The owner's report, as a test (RD-120-23): the form offered accounts whose service
   * cannot take a job at all, and said so only after the button was pressed.
   *
   * Two providers with a plugin and one without, because with a single provider a correct
   * row and a wrong one look the same -- every line would be true and false at once.
   */
  it('offers only accounts whose service has a remote-job plugin', async () => {
    jobs.value = []
    mount([ACCOUNT, PREMIUMIZE, DDOWNLOAD])
    const select = await waitFor(() => screen.getByRole('combobox'))
    await fireEvent.click(select)
    await waitFor(() => expect(screen.getByText('Real-Debrid · realdebrid')).toBeTruthy())
    expect(screen.getByText('Premiumize · premiumize')).toBeTruthy()
    expect(screen.queryByText('DDownload · ddownload')).toBeNull()
  })

  /**
   * And the answer comes from the server, not from a list in the client: the same accounts
   * with a different answer are offered differently.
   */
  it('takes the answer from the installed plugins and not from a list of its own', async () => {
    jobs.value = []
    // The same three accounts, a different set of installed plugins, a different offer.
    mount([ACCOUNT, PREMIUMIZE, DDOWNLOAD], ['ddownload'])
    const select = await waitFor(() => screen.getByRole('combobox'))
    await fireEvent.click(select)
    await waitFor(() => expect(screen.getByText('DDownload · ddownload')).toBeTruthy())
    expect(screen.queryByText('Real-Debrid · realdebrid')).toBeNull()
    expect(screen.queryByText('Premiumize · premiumize')).toBeNull()
  })

  it('says so instead of showing an empty selection when no account fits', async () => {
    jobs.value = []
    mount([DDOWNLOAD])
    await waitFor(() => expect(screen.getByText(en.no_remote_job_accounts)).toBeTruthy())
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.queryByRole('button', { name: en.submit.action })).toBeNull()
  })

  it('says the list could not be read rather than filtering on a guess', async () => {
    jobs.value = []
    mount([ACCOUNT], null)
    await waitFor(() => expect(screen.getByText(en.accounts_unavailable)).toBeTruthy())
    expect(screen.queryByRole('combobox')).toBeNull()
  })

  /**
   * RD-120-31: a provider plugin took a container, and nothing ever handed it one. The file
   * goes as base64 in the same request, and it replaces the magnet rather than riding along.
   */
  it('hands a .torrent over as base64 instead of a magnet', async () => {
    jobs.value = []
    mount()
    const select = await waitFor(() => screen.getByRole('combobox'))
    await waitFor(() => screen.getByRole('option', { name: 'Real-Debrid · realdebrid' }))
    await fireEvent.update(select, 'a1')
    const input = document.querySelector('[data-testid="remote-job-file"]') as HTMLInputElement
    Object.defineProperty(input, 'files', { value: [new File(['d4:infoe'], 'bbb.torrent')], configurable: true })
    await fireEvent.change(input)
    await waitFor(() => expect(screen.getByText('bbb.torrent')).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: en.submit.action }))

    await waitFor(() => expect(posts.length).toBe(1))
    expect(posts[0]?.path).toBe('/api/v1/accounts/{id}/remote-jobs')
    expect(posts[0]?.init).toMatchObject({ params: { path: { id: 'a1' } }, body: { container: btoa('d4:infoe') } })
    expect((posts[0]?.init as { body: object }).body).not.toHaveProperty('magnet')
  })

  it('refuses a file too large for a remote job before reading or sending it', async () => {
    jobs.value = []
    const view = mount()
    await waitFor(() => screen.getByRole('combobox'))
    const input = document.querySelector('[data-testid="remote-job-file"]') as HTMLInputElement
    const file = new File(['x'], 'huge.nzb')
    Object.defineProperty(file, 'size', { value: 16 * 1024 * 1024 + 1 })
    Object.defineProperty(input, 'files', { value: [file], configurable: true })
    await fireEvent.change(input)

    // Listed with its reason, so the reader sees which file it was; never read, never sent.
    await waitFor(() => expect(screen.getByText('huge.nzb')).toBeTruthy())
    expect(screen.getByText(en.submit.file_too_large.replace('{max_mib}', '16'))).toBeTruthy()
    expect(screen.getByText(en.files.states.refused)).toBeTruthy()
    expect((screen.getByRole('button', { name: en.submit.action }) as HTMLButtonElement).disabled).toBe(true)
    expect(view.emitted('error')).toBeUndefined()
    expect(posts).toEqual([])
  })

  it('removes a row from the list without ever reaching the provider', async () => {
    jobs.value = [WORKING]
    mount()
    const button = await waitFor(() => screen.getByRole('button', { name: en.forget.action }))
    await fireEvent.click(button)
    await waitFor(() => expect(deletes).toEqual(['/api/v1/remote-jobs/{id}']))
    expect(posts).toEqual([])
  })

  /**
   * RD-120-51: the owner opened the page and found an empty picker. While the list of
   * services is on its way the form says what it is waiting for, and draws no picker at all.
   */
  it('shows why it waits instead of an empty picker while the providers are read', async () => {
    jobs.value = []
    let open: () => void = () => {}
    providersGate.value = new Promise(resolve => { open = resolve })
    mount()
    await waitFor(() => expect(screen.getByText(en.accounts_loading)).toBeTruthy())
    expect(screen.getByTestId('remote-job-accounts-loading').getAttribute('role')).toBe('status')
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.queryByText(en.no_remote_job_accounts)).toBeNull()

    open()
    providersGate.value = null
    await waitFor(() => expect(screen.getByRole('combobox')).toBeTruthy())
    expect(screen.queryByText(en.accounts_loading)).toBeNull()
  })

  it('waits for the accounts too, rather than saying none of them fits', async () => {
    jobs.value = []
    posts.length = 0
    providers.value = ['realdebrid']
    mountComponent(SettingsRemoteJobsCard, {
      messages: { remote_jobs: en, server },
      props: { accounts: [], accountsLoading: true }
    })
    await waitFor(() => expect(screen.getByText(en.accounts_loading)).toBeTruthy())
    expect(screen.queryByText(en.no_remote_job_accounts)).toBeNull()
  })

  /**
   * RD-120-51: several files at once. Each is its own request, strictly one after another;
   * one the server refuses keeps its reason and the rest are sent regardless; a file of the
   * wrong type is listed with its reason and never read.
   */
  it('hands several files over one after another, and a failure stops none of the rest', async () => {
    jobs.value = []
    let release: () => void = () => {}
    const held = new Promise<void>(resolve => { release = resolve })
    const view = mount()
    submitAnswers.push(
      async () => {
        await held
        return { data: { job: { ...WORKING, id: 'j10' }, already_running: false } }
      },
      { error: { code: 'container.unrecognised' } },
      { data: { job: { ...WORKING, id: 'j12' }, already_running: true } }
    )
    const select = await waitFor(() => screen.getByRole('combobox'))
    await waitFor(() => screen.getByRole('option', { name: 'Real-Debrid · realdebrid' }))
    await fireEvent.update(select, 'a1')
    const input = document.querySelector('[data-testid="remote-job-file"]') as HTMLInputElement
    const picked = ['one.torrent', 'two.nzb', 'notes.txt', 'three.torrent'].map(name => new File(['d4:infoe'], name))
    Object.defineProperty(input, 'files', { value: picked, configurable: true })
    await fireEvent.change(input)
    await waitFor(() => expect(screen.getByText('three.torrent')).toBeTruthy())
    expect(screen.getByText(en.files.wrong_type)).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: en.submit.action }))
    // The first request is still open, and nothing else has gone out.
    await waitFor(() => expect(posts.length).toBe(1))
    expect(screen.getByText(en.files.states.sending)).toBeTruthy()
    release()

    await waitFor(() => expect(posts.length).toBe(3))
    expect(posts.every(post => post.path === '/api/v1/accounts/{id}/remote-jobs')).toBe(true)
    expect(posts.map(post => (post.init as { params: { path: { id: string } } }).params.path.id)).toEqual(['a1', 'a1', 'a1'])
    await waitFor(() => expect(screen.getByText(en.files.states.already_running)).toBeTruthy())
    expect(screen.getByText(en.files.states.started)).toBeTruthy()
    expect(screen.getByText(en.files.states.failed)).toBeTruthy()
    expect(screen.getByText('container.unrecognised')).toBeTruthy()
    expect(screen.getByText(en.files.states.refused)).toBeTruthy()
    expect(view.emitted('error')?.[0]).toEqual([
      en.files.summary_failed.replace('{failed}', '1').replace('{total}', '3')
    ])
    // Both jobs the server answered with are in the list.
    await waitFor(() => expect(screen.getAllByText(en.states.working).length).toBe(2))
  })

  it('takes files dropped on the form like chosen ones', async () => {
    jobs.value = []
    mount()
    await waitFor(() => screen.getByRole('combobox'))
    const zone = screen.getByTestId('remote-job-drop')
    const dropped = [new File(['a'], 'a.torrent'), new File(['b'], 'b.nzb')]
    await fireEvent.drop(zone, { dataTransfer: { files: dropped, types: ['Files'] } })
    await waitFor(() => expect(screen.getByText('b.nzb')).toBeTruthy())
    expect(screen.getByText('a.torrent')).toBeTruthy()
    expect(screen.getAllByText(en.files.states.waiting).length).toBe(2)
  })
})
