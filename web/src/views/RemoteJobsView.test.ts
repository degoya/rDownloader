/**
 * Remote jobs as a place of their own (RD-110-29): the card that used to sit under Accounts,
 * unchanged, under a page header, fed with the accounts it needs.
 */
import { screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent } from 'vue'

import nav from '@/locales/en/nav.json'
import remoteJobs from '@/locales/en/remote_jobs.json'
import { mountComponent } from '@/test/mount'

const ACCOUNT = { id: 'a1', provider: 'realdebrid', label: 'Real-Debrid', enabled: true }
const get = vi.fn(async () => ({ data: [ACCOUNT] }))
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...(args as [])) },
  responseError: () => 'failed'
}))
// The card, even stubbed, is imported by the view and pulls the Nuxt UI barrel in.
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }) })
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))

/** Records what the card is handed, so the test asserts the hand-over and not the card itself. */
const cardProps: { accounts: unknown, accountsLoading?: boolean }[] = []
const SettingsRemoteJobsCard = defineComponent({
  props: { accounts: { type: Array, required: true }, accountsLoading: { type: Boolean, default: false } },
  setup(props) {
    cardProps.push(props)
  },
  template: '<div data-testid="card" />'
})

import RemoteJobsView from './RemoteJobsView.vue'

function mount() {
  cardProps.length = 0
  return mountComponent(RemoteJobsView, {
    messages: { nav, remote_jobs: remoteJobs },
    stubs: { SettingsRemoteJobsCard }
  })
}

describe('RemoteJobsView', () => {
  it('opens with the page header every page carries', () => {
    mount()
    const heading = screen.getByRole('heading', { level: 2 })
    expect(heading.textContent?.trim()).toBe(remoteJobs.page.title)
    expect(screen.getByText(remoteJobs.page.description)).toBeTruthy()
    expect(screen.getByText(remoteJobs.page.eyebrow)).toBeTruthy()
  })

  it('renders the card and hands it the accounts it fetched', async () => {
    mount()
    expect(screen.getByTestId('card')).toBeTruthy()
    expect(get).toHaveBeenCalledWith('/api/v1/accounts')
    await waitFor(() => expect(cardProps[0]?.accounts).toEqual([ACCOUNT]))
  })

  /**
   * RD-120-51: an empty account list that is still on its way is not an answer. The card is
   * told so, and only the arrival of the list ends it.
   */
  it('tells the card while the accounts are still being read', async () => {
    let arrive: (value: { data: (typeof ACCOUNT)[] }) => void = () => {}
    get.mockImplementationOnce(() => new Promise(resolve => { arrive = resolve }))
    mount()
    expect(cardProps[0]?.accountsLoading).toBe(true)
    arrive({ data: [ACCOUNT] })
    await waitFor(() => expect(cardProps[0]?.accountsLoading).toBe(false))
    expect(cardProps[0]?.accounts).toEqual([ACCOUNT])
  })
})
