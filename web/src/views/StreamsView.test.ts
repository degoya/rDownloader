import { fireEvent, render, screen, waitFor, within } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import common from '@/locales/en/common.json'
import en from '@/locales/en/streams.json'
import { useStreamsStore } from '@/stores/streams'
import { axeViolations } from '@/test/axe'
import { uiStubs } from '@/test/mount'

import StreamsView from './StreamsView.vue'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn()
}))
const confirm = vi.hoisted(() => vi.fn())
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
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
        UCheckboxGroup: uiStubs.UCheckboxGroup,
        UDashboardNavbar: passthrough,
        UDashboardPanel: {
          template: '<div><slot name="header" /><slot name="body" /></div>'
        },
        UDashboardSidebarCollapse: true,
        UEmpty: uiStubs.UEmpty,
        UFileUpload: uiStubs.UFileUpload,
        UFormField: uiStubs.UFormField,
        UIcon: true,
        UInput: { props: ['modelValue'], emits: ['update:modelValue'], template: '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />' },
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
    post.mockReset()
  })

  it('asks for a channel instead of showing an unusable schedule form', async () => {
    mount()
    // Without a channel the picker is empty, but the submit button used to stay live and
    // posted an empty channel id that the server rejected. The form must not be reachable.
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())
    expect(screen.queryByTestId('schedule-name')).toBeNull()
    expect(screen.queryByTestId('schedule-actions')).toBeNull()
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

  it('asks before a schedule is deleted, and keeps it when the answer is no', async () => {
    mount()
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())
    const store = useStreamsStore()
    const removeSchedule = vi.spyOn(store, 'removeSchedule').mockResolvedValue()
    // The schedule list sits behind at least one channel.
    store.channels = [{ id: 'x', name: 'Example channel', url: 'https://example.com/live', enabled: true } as never]
    store.schedules = [
      { id: '019d0000-0000-7000-8000-000000000002', name: 'Evening show', channel_id: 'x', timezone: 'UTC' } as never
    ]
    // The last Delete on the page is the schedule's; the channels above have their own.
    const button = (await screen.findAllByRole('button', { name: common.actions.delete })).at(-1) as HTMLElement

    confirm.mockResolvedValueOnce(false)
    await fireEvent.click(button)
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(removeSchedule).not.toHaveBeenCalled()

    confirm.mockResolvedValueOnce(true)
    await fireEvent.click(button)
    await waitFor(() => expect(removeSchedule).toHaveBeenCalledWith('019d0000-0000-7000-8000-000000000002'))
  })

  it('picks weekdays in a named group of checkboxes, not in a row of unlabelled buttons', async () => {
    mount()
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())
    const store = useStreamsStore()
    store.channels = [{ id: 'x', name: 'Example channel', url: 'https://example.com/live', enabled: true } as never]

    const group = await screen.findByRole('group', { name: en.schedules.days })
    const monday = within(group).getByRole('checkbox', { name: en.schedules.weekday['1'] }) as HTMLInputElement
    expect(monday.checked).toBe(false)
    await fireEvent.click(monday)
    expect(monday.checked).toBe(true)
  })

  it('keeps the channel form a form whose action row starts with the create button', async () => {
    mount()
    const form = await screen.findByTestId('channel-form')
    // The URL decides everything else about a channel; it is the first field.
    expect(form.querySelector('input')?.getAttribute('placeholder')).toBe('https://twitch.tv/channel')
    const submit = within(form).getByRole('button', { name: en.form.create })
    expect(submit.getAttribute('type')).toBe('submit')
    expect(form.querySelector('[data-form-actions] button')).toBe(submit)
  })

  it('duplicates a schedule with its settings under a new name and opens the copy for editing', async () => {
    mount()
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())
    const store = useStreamsStore()
    store.channels = [{ id: 'x', name: 'Example channel', url: 'https://example.com/live', enabled: true } as never]
    const original = {
      id: 's1', name: 'Evening show', channel_id: 'x', enabled: true, kind: 'weekly', days: [1, 3],
      start_minute: 20 * 60, timezone: 'Europe/Berlin', window_minutes: 90, lead_minutes: 5,
      trail_minutes: 15, replay_from_start: true
    }
    store.schedules = [original as never]
    const copy = { ...original, id: 's2', name: `Evening show (${common.copy_suffix})` }
    post.mockResolvedValueOnce({ data: copy })
    get.mockImplementation(async (path: string) =>
      path === '/api/v1/streams/schedules' ? { data: [original, copy] } : { data: [] }
    )

    await fireEvent.click(await screen.findByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    const body = post.mock.calls[0]?.[1]?.body
    expect(body).toMatchObject({
      channel_id: 'x', name: `Evening show (${common.copy_suffix})`, days: [1, 3], start_minute: 1200,
      timezone: 'Europe/Berlin', window_minutes: 90, lead_minutes: 5, trail_minutes: 15, replay_from_start: true
    })
    expect(body).not.toHaveProperty('id')
    await waitFor(() => expect((screen.getByTestId('schedule-name') as HTMLInputElement).value).toBe(copy.name))
    const rows = screen.getAllByTestId('schedule-row')
    expect(within(rows[1] as HTMLElement).getByText(common.editing)).toBeTruthy()
    expect(within(rows[0] as HTMLElement).queryByText(common.editing)).toBeNull()
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(en.schedules.needs_channel)).toBeTruthy())
    expect(await axeViolations(container)).toBe('')
  })

})
