/**
 * A package's download window (RD-1240-30): which window applies, what the dialog hands back,
 * and what the package row shows and offers.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import type { Category, DownloadPackage } from '@/api/types'
import bandwidth from '@/locales/en/bandwidth.json'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { useQueuePauseStore } from '@/stores/queuePause'
import { mountComponent, passthrough } from '@/test/mount'

import PackageDownloadWindowModal from '@/components/PackageDownloadWindowModal.vue'
import PackageGroup from '@/components/PackageGroup.vue'

import { effectiveWindow } from './usePackageDownloadWindow'

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }), useOverlay: () => ({ create: vi.fn() }) }))
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), PUT: vi.fn() },
  responseError: () => 'failed',
  errorMessage: () => 'failed'
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: vi.fn(() => () => {}) }))

/** The modal's body and footer, which the shared `passthrough` stub leaves out. */
const UModal = { template: '<div><slot name="body" /><slot name="footer" /></div>' }

const night = { windows: [{ days: 127, start_minute: 22 * 60, end_minute: 6 * 60 }], ignore_schedule_pause: false }

function pkg(overrides: Record<string, unknown> = {}): DownloadPackage {
  return {
    id: 'package-1', name: 'Tonight', state: 'queued', destination: '/downloads/Tonight', category_id: 'cat',
    priority: 'normal', position: 1, has_password: false, kind: 'http', nzb_import_id: null,
    created_at: '2026-09-02T10:00:00Z', enrichment: [], start_after: null, download_window: null,
    ...overrides
  } as unknown as DownloadPackage
}

const category = { id: 'cat', name: 'Night', download_window: night } as unknown as Category

afterEach(() => vi.useRealTimers())

describe('the window that applies', () => {
  it('is the package’s own, otherwise its category’s', () => {
    const own = { windows: [], ignore_schedule_pause: true }
    expect(effectiveWindow(pkg({ download_window: own }), [category])).toEqual(own)
    expect(effectiveWindow(pkg(), [category])).toEqual(night)
    expect(effectiveWindow(pkg({ category_id: null }), [category])).toBeNull()
  })
})

describe('PackageDownloadWindowModal', () => {
  it('starts a window with the night and hands it back', async () => {
    const { container, emitted } = mountComponent(PackageDownloadWindowModal, {
      messages: { downloads, common, bandwidth },
      props: { name: 'Tonight', current: null, categoryWindow: null, timezone: 'Europe/Berlin' },
      stubs: { UModal }
    })
    await fireEvent.click(screen.getByRole('switch', { name: downloads.window.own_label }))
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    const [[result]] = emitted('close') as [[{ window: unknown }]]
    expect(result.window).toEqual(night)
  })

  it('hands back none once the package’s own window is switched off', async () => {
    const { container, emitted } = mountComponent(PackageDownloadWindowModal, {
      messages: { downloads, common, bandwidth },
      props: { name: 'Tonight', current: night, categoryWindow: null, timezone: 'UTC' },
      stubs: { UModal }
    })
    await fireEvent.click(screen.getByRole('switch', { name: downloads.window.own_label }))
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    const [[result]] = emitted('close') as [[{ window: unknown }]]
    expect(result.window).toBeNull()
  })
})

describe('the package row', () => {
  it('warns while the category’s window is closed and offers the dialog', async () => {
    vi.useFakeTimers({ now: new Date('2026-01-15T12:00:00Z'), toFake: ['Date'] })
    mountComponent(PackageGroup, {
      messages: { downloads, common },
      props: {
        package: pkg(), downloads: [], categories: [category], selection: 'none', open: false, complete: false,
        packageRate: 0, packageEta: null, dragging: false, canPause: false, canResume: false, controlBusy: null
      },
      stubs: { UBadge: passthrough }
    })
    useQueuePauseStore().scheduleTimezone = 'UTC'
    await nextTick()
    const glyph = screen.getByTestId('download-window')
    expect(glyph.getAttribute('aria-label')).toBe(downloads.window.glyph)
    expect(glyph.getAttribute('color')).toBe('warning')
    expect(screen.getByRole('button', { name: downloads.window.menu })).toBeTruthy()
  })

  it('shows no glyph for a package no window applies to', () => {
    mountComponent(PackageGroup, {
      messages: { downloads, common },
      props: {
        package: pkg({ category_id: null }), downloads: [], categories: [], selection: 'none', open: false, complete: false,
        packageRate: 0, packageEta: null, dragging: false, canPause: false, canResume: false, controlBusy: null
      },
      stubs: { UBadge: passthrough }
    })
    expect(screen.queryByTestId('download-window')).toBeNull()
  })
})
