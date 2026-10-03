/**
 * An automation's version history (RD-190-22): newest first, what each version changed, and a
 * restore that goes through the ordinary update and so becomes the newest version.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Automation, AutomationVersion } from '@/api/types'
import automation from '@/locales/en/automation.json'
import { mountComponent } from '@/test/mount'

const get = vi.hoisted(() => vi.fn())
const put = vi.hoisted(() => vi.fn())
const confirm = vi.hoisted(() => vi.fn(async () => true))
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    PUT: (...args: unknown[]) => put(...args),
    POST: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: () => 'The service did not answer'
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))

const { default: AutomationVersionsModal } = await import('./AutomationVersionsModal.vue')

const AUTOMATION = {
  id: 'a1',
  name: 'Tidy up',
  enabled: true,
  version: 2,
  created_at: '2026-10-01T08:00:00Z',
  updated_at: '2026-10-02T08:00:00Z',
  definition: null
} as unknown as Automation

function version(number: number, part: Partial<AutomationVersion>): AutomationVersion {
  return {
    id: `v${number}`,
    automation_id: 'a1',
    version: number,
    trigger: 'download_completed',
    condition: { type: 'always' },
    actions: [{ kind: 'pause_package' }],
    created_at: `2026-10-0${number}T08:00:00Z`,
    ...part
  }
}

const V1 = version(1, { actions: [{ kind: 'set_category', category_id: 'c1' }] })
const V2 = version(2, {})

function mount() {
  return mountComponent(AutomationVersionsModal, {
    messages: { automation },
    props: { automation: AUTOMATION, categories: [{ id: 'c1', name: 'Movies' }], targets: [] },
    stubs: { UModal: { props: ['title'], template: '<div>{{ title }}<slot name="body" /></div>' } }
  })
}

describe('AutomationVersionsModal', () => {
  beforeEach(() => {
    get.mockReset()
    put.mockReset()
    confirm.mockClear()
  })

  it('lists the versions newest first and marks what each one changed', async () => {
    get.mockResolvedValue({ data: [V1, V2] })
    mount()
    const rows = await screen.findAllByTestId('automation-version')
    expect(rows.map(row => row.textContent)).toEqual([
      expect.stringContaining('Version 2'),
      expect.stringContaining('Version 1')
    ])
    expect(rows[0]?.textContent).toContain(automation.history.current)
    expect(rows[0]?.textContent).toContain(automation.history.changed)
    // The first version changed nothing against a version before it, and names its category.
    expect(rows[1]?.textContent).not.toContain(automation.history.changed)
    expect(rows[1]?.textContent).toContain(`${automation.action.set_category}: Movies`)
    expect(get).toHaveBeenCalledWith('/api/v1/automations/{id}/versions', { params: { path: { id: 'a1' } } })
  })

  it('restores an older version through the update, keeping name and switch', async () => {
    get.mockResolvedValueOnce({ data: [V1, V2] })
      .mockResolvedValue({ data: [] })
    put.mockResolvedValue({ data: { ...AUTOMATION, version: 3 } })
    const { emitted } = mount()
    await screen.findAllByTestId('automation-version')
    // The current version offers no restore; the older one does.
    expect(screen.getAllByRole('button', { name: automation.history.restore })).toHaveLength(1)

    get.mockResolvedValue({ data: [V1, V2, version(3, V1)] })
    await fireEvent.click(screen.getByRole('button', { name: automation.history.restore }))

    await waitFor(() => expect(put).toHaveBeenCalledWith('/api/v1/automations/{id}', {
      params: { path: { id: 'a1' } },
      body: { name: 'Tidy up', enabled: true, trigger: V1.trigger, condition: V1.condition, actions: V1.actions }
    }))
    expect(confirm).toHaveBeenCalledOnce()
    await waitFor(() => expect(emitted().restored).toEqual([['a1']]))
    expect(await screen.findByText(/Version 1 is in force again, saved as version 3/)).not.toBeNull()
  })
})
