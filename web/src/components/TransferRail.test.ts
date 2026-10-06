/**
 * The selection figure in the status bar (RD-170-14): shown while a list has something ticked,
 * gone when it has not, and a lower bound where a size is still unknown. The parallel downloads
 * set from the bar (RD-1120-22) at the end.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(),
  errorMessage: vi.fn()
}))
const toastAdd = vi.hoisted(() => vi.fn())
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toastAdd }) }))

import { api, responseError } from '@/api/client'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { useSelectionStore } from '@/stores/selection'
import { useTransfersStore } from '@/stores/transfers'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import TransferRail from './TransferRail.vue'

function mount() {
  mountComponent(TransferRail, { messages: { downloads }, stubs: { SpeedHistoryChart: true } })
  return useSelectionStore()
}

describe('TransferRail selection', () => {
  it('is hidden while nothing is selected', () => {
    mount()
    expect(screen.queryByTestId('rail-selection')).toBeNull()
  })

  it('shows the count and the summed size, and goes when the selection empties', async () => {
    const store = mount()
    const owner = Symbol('view')
    store.publish(owner, { count: 3, bytes: 5n * 1024n ** 3n, unknown: 0 })
    await nextTick()
    const figure = screen.getByTestId('rail-selection')
    expect(figure.textContent?.replace(/\s+/g, ' ').trim()).toBe('3 selected · 5.0 GiB')
    expect(figure.getAttribute('title')).toBe('3 selected, 5.0 GiB in total')

    store.release(owner)
    await nextTick()
    expect(screen.queryByTestId('rail-selection')).toBeNull()
  })

  it('marks a sum that leaves out unknown sizes as a lower bound', async () => {
    const store = mount()
    store.publish(Symbol('view'), { count: 4, bytes: 2048n, unknown: 2 })
    await nextTick()
    const figure = screen.getByTestId('rail-selection')
    expect(figure.textContent).toContain('≥ 2.0 KiB')
    expect(figure.getAttribute('title')).toBe('4 selected, at least 2.0 KiB in total – the size of 2 is not known yet')
  })

  it('shows only the count when no size is known at all', async () => {
    const store = mount()
    store.publish(Symbol('view'), { count: 2, bytes: 0n, unknown: 2 })
    await nextTick()
    const figure = screen.getByTestId('rail-selection')
    expect(figure.textContent).not.toContain('·')
    expect(figure.getAttribute('title')).toBe('2 selected, size not known yet')
  })
})

/**
 * RD-1120-22: how many downloads run at once is set in the status bar, beside how many do, and
 * saved through the same settings write as the speed limit.
 */
describe('TransferRail parallel downloads', () => {
  const stored = { speed_limit_bytes_per_second: null, max_active_files: 3, max_retries: 8 }

  beforeEach(() => {
    toastAdd.mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.PUT).mockReset()
    vi.mocked(api.GET).mockImplementation((async (path: string) =>
      path === '/api/v1/settings' ? { data: { ...stored } } : { data: undefined }) as never)
    vi.mocked(api.PUT).mockImplementation((async (_path: string, init: { body: unknown }) => ({ data: init.body })) as never)
  })

  function field(): HTMLInputElement {
    return screen.getByLabelText(downloads.rail.parallel_limit_aria) as HTMLInputElement
  }

  async function mounted() {
    const view = mountComponent(TransferRail, { messages: { downloads }, stubs: { SpeedHistoryChart: true } })
    await waitFor(() => expect(field().value).toBe('3'))
    return view
  }

  it('shows the stored number with a label for screen readers, and nothing to apply yet', async () => {
    await mounted()
    expect(screen.getByTestId('rail-parallel-limit').className).toContain('@min-[56rem]:flex')
    expect(screen.queryByRole('button', { name: common.actions.apply })).toBeNull()
  })

  it('saves a changed number into the whole stored document', async () => {
    await mounted()
    await fireEvent.update(field(), '1')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.apply }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(1))
    const [path, init] = vi.mocked(api.PUT).mock.calls[0] as unknown as [string, { body: typeof stored }]
    expect(path).toBe('/api/v1/settings')
    expect(init.body).toEqual({ ...stored, max_active_files: 1 })
    await waitFor(() => expect(screen.queryByRole('button', { name: common.actions.apply })).toBeNull())
    expect(toastAdd).not.toHaveBeenCalled()
  })

  it.each([0, 33, 2.5])('refuses %s without a request, with a toast', async (value) => {
    await mounted()
    await fireEvent.update(field(), String(value))
    await fireEvent.keyUp(field(), { key: 'Enter' })

    await waitFor(() => expect(toastAdd).toHaveBeenCalledTimes(1))
    expect(api.PUT).not.toHaveBeenCalled()
    expect(toastAdd.mock.calls[0]?.[0]).toMatchObject({ title: downloads.rail.parallel_failed, description: 'Between 1 and 32 downloads may run at once.', color: 'error' })
    expect(field().value).toBe('3')
  })

  it('raises the refusal of the service as a toast and goes back to the stored number', async () => {
    vi.mocked(api.PUT).mockImplementation((async () => ({ data: undefined, error: { code: 'settings.invalid' } })) as never)
    vi.mocked(responseError).mockReturnValue('The settings were refused')
    await mounted()
    await fireEvent.update(field(), '5')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.apply }))

    await waitFor(() => expect(toastAdd).toHaveBeenCalledTimes(1))
    expect(toastAdd.mock.calls[0]?.[0]).toMatchObject({ title: downloads.rail.parallel_failed, description: 'The settings were refused' })
    expect(field().value).toBe('3')
  })

  it('follows a value the settings page loaded or saved', async () => {
    await mounted()
    useTransfersStore().applyRailSettings({ speed_limit_bytes_per_second: null, max_active_files: 7 })
    await waitFor(() => expect(field().value).toBe('7'))
  })

  it('renders the bar without an axe violation', async () => {
    const { container } = await mounted()
    expect(await axeViolations(container)).toBe('')
  })
})
