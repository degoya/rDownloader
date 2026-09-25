import { render, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import common from '@/locales/en/common.json'
import en from '@/locales/en/streams.json'
import { useStreamsStore } from '@/stores/streams'

import StreamsView from './StreamsView.vue'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn()
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn() }))
// The export/import buttons pull in Nuxt UI's toast composable, whose runtime path resolves
// through `#imports` and does not exist outside a Nuxt build. Mocked the same way
// `IndexerReviewList.test.ts` does.
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const i18n = createI18n({
  legacy: false,
  locale: 'en',
  messages: { en: { streams: en, common } }
})

/** Renders slot content so the sections under test are reachable. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }

function mount() {
  return render(StreamsView, {
    global: {
      plugins: [i18n],
      stubs: {
        UAlert: true,
        UBadge: passthrough,
        UButton: { props: ['label'], template: '<button v-bind="$attrs">{{ label }}<slot /></button>' },
        UCheckbox: true,
        UDashboardNavbar: passthrough,
        UDashboardPanel: {
          template: '<div><slot name="header" /><slot name="body" /></div>'
        },
        UDashboardSidebarCollapse: true,
        UFormField: passthrough,
        UIcon: true,
        UInput: { props: ['modelValue'], template: '<input v-bind="$attrs" :value="modelValue" />' },
        USelect: { props: ['modelValue', 'items'], template: '<select v-bind="$attrs" />' },
        USwitch: true
      }
    }
  })
}

describe('StreamsView schedules', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    get.mockReset()
    get.mockResolvedValue({ data: [] })
  })

  it('asks for a channel instead of showing an unusable schedule form', async () => {
    mount()
    // Without a channel the picker is empty, but the submit button used to stay live and
    // posted an empty channel id that the server rejected. The form must not be reachable.
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())
    expect(screen.queryByTestId('schedule-name')).toBeNull()
    expect(screen.queryByTestId('schedule-submit')).toBeNull()
  })

  it('shows the schedule form once a channel exists', async () => {
    mount()
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())

    const store = useStreamsStore()
    store.channels = [
      {
        id: '019d0000-0000-7000-8000-000000000001',
        name: 'Example channel',
        url: 'https://example.com/live',
        enabled: true
      } as never
    ]

    await waitFor(() => expect(screen.getByTestId('schedule-name')).toBeTruthy())
    expect(screen.queryByText(en.schedules.needs_channel)).toBeNull()
  })
})
