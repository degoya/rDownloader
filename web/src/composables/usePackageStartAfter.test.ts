/**
 * A package's "not before" (RD-1240-14): which moment still holds, what the dialog opens on and
 * hands back, and what the package row shows and offers.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { DownloadPackage } from '@/api/types'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent, passthrough } from '@/test/mount'

import PackageGroup from '@/components/PackageGroup.vue'
import PackageStartAfterModal from '@/components/PackageStartAfterModal.vue'

import { pendingStartAfter, startAfterFields, startAfterMoment } from './usePackageStartAfter'

const toasts = vi.hoisted(() => vi.fn())
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toasts }), useOverlay: () => ({ create: vi.fn() }) }))
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), PUT: vi.fn() },
  responseError: () => 'failed',
  errorMessage: () => 'failed'
}))

/** The modal's body and footer, which the shared `passthrough` stub leaves out. */
const UModal = { template: '<div><slot name="body" /><slot name="footer" /></div>' }

beforeEach(() => {
  toasts.mockReset()
  vi.mocked(api.PUT).mockReset()
})
afterEach(() => vi.useRealTimers())

describe('the start-time helpers', () => {
  it('keeps only a moment that lies ahead', () => {
    const now = Date.parse('2026-10-10T12:00:00Z')
    expect(pendingStartAfter('2026-10-10T13:00:00Z', now)).toBe('2026-10-10T13:00:00Z')
    expect(pendingStartAfter('2026-10-10T11:00:00Z', now)).toBeNull()
    expect(pendingStartAfter(null, now)).toBeNull()
    expect(pendingStartAfter(undefined, now)).toBeNull()
  })

  it('reads a day and a clock in this browser and refuses half a moment', () => {
    const moment = startAfterMoment('2026-10-11', '02:30')
    expect([moment?.getFullYear(), moment?.getMonth(), moment?.getDate(), moment?.getHours(), moment?.getMinutes()]).toEqual([2026, 9, 11, 2, 30])
    expect(startAfterMoment('', '02:30')).toBeNull()
    expect(startAfterMoment('2026-10-11', '')).toBeNull()
  })

  it('opens on the stored moment, or on the next full hour, past midnight too', () => {
    const stored = new Date(2026, 9, 11, 2, 30).toISOString()
    expect(startAfterFields(stored)).toEqual({ day: '2026-10-11', clock: '02:30' })
    expect(startAfterFields(null, new Date(2026, 9, 10, 23, 40))).toEqual({ day: '2026-10-11', clock: '00:00' })
  })
})

describe('PackageStartAfterModal', () => {
  it('hands back the chosen moment and refuses one that has passed', async () => {
    vi.useFakeTimers({ now: new Date(2026, 9, 10, 12, 15), toFake: ['Date'] })
    const { container, emitted } = mountComponent(PackageStartAfterModal, {
      messages: { downloads, common },
      props: { name: 'Tonight', current: null },
      stubs: { UModal }
    })
    expect(screen.getByTestId('start-after-summary').textContent).toContain('Starts not before')

    await fireEvent.update(screen.getByTestId('start-after-time'), '09:00')
    expect(screen.getByTestId('start-after-summary').textContent).toContain(downloads.start_after.not_ahead)
    expect((screen.getByTestId('start-after-save') as HTMLButtonElement).disabled).toBe(true)

    await fireEvent.update(screen.getByTestId('start-after-time'), '22:45')
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    const [[result]] = emitted('close') as [[{ at: string }]]
    expect(new Date(result.at).getTime()).toBe(new Date(2026, 9, 10, 22, 45).getTime())
  })
})

function renderRow(startAfter: string | null) {
  return mountComponent(PackageGroup, {
    messages: { downloads, common },
    props: {
      package: {
        id: 'package-1', name: 'Tonight', state: 'queued', destination: '/downloads/Tonight', category_id: null,
        priority: 'normal', position: 1, has_password: false, kind: 'http', nzb_import_id: null,
        created_at: '2026-09-02T10:00:00Z', enrichment: [], start_after: startAfter
      } as unknown as DownloadPackage,
      downloads: [], categories: [], selection: 'none', open: false, complete: false,
      packageRate: 0, packageEta: null, dragging: false, canPause: false, canResume: false, controlBusy: null
    },
    stubs: { UBadge: passthrough }
  })
}

describe('the package row', () => {
  it('shows the glyph while the moment lies ahead and offers to remove it', async () => {
    const later = new Date(Date.now() + 3_600_000).toISOString()
    vi.mocked(api.PUT).mockResolvedValueOnce({ data: { package_id: 'package-1', start_after: null } } as never)
    renderRow(later)
    expect(screen.getByTestId('start-after').getAttribute('aria-label')).toBe(downloads.start_after.glyph)
    expect(screen.getByRole('button', { name: downloads.start_after.set })).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: downloads.start_after.clear }))
    await vi.waitFor(() => expect(toasts).toHaveBeenCalled())
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/packages/{id}/start-after', {
      params: { path: { id: 'package-1' } },
      body: { start_after: null }
    })
    expect(toasts.mock.calls[0]?.[0]).toMatchObject({ title: downloads.start_after.cleared, color: 'success' })
  })

  it('shows nothing for a moment that has passed and offers only to set one', () => {
    renderRow(new Date(Date.now() - 60_000).toISOString())
    expect(screen.queryByTestId('start-after')).toBeNull()
    expect(screen.getByRole('button', { name: downloads.start_after.set })).toBeTruthy()
    expect(screen.queryByRole('button', { name: downloads.start_after.clear })).toBeNull()
  })
})
