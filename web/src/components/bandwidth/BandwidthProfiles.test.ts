/**
 * The profile list names everything a profile limits (RD-120-53).
 *
 * Found in the second screenshot run: a profile holding nothing but a 1 TiB monthly budget read
 * "Unlimited · 0 scope limits", because the line carried only the download rate and the number
 * of scope limits. The budget was stored — the API returned it — and the list dropped it.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { BandwidthProfile } from '@/api/types'
import bandwidth from '@/locales/en/bandwidth.json'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: () => ''
}))
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }) })
}))

import BandwidthProfiles from './BandwidthProfiles.vue'

function profile(overrides: Partial<BandwidthProfile>): BandwidthProfile {
  return {
    id: 'profile',
    name: 'Profile',
    download_bytes_per_second: null,
    upload_bytes_per_second: null,
    max_active_files: null,
    daily_budget_bytes: null,
    monthly_budget_bytes: null,
    scopes: [],
    ...overrides
  } as BandwidthProfile
}

function mount(profiles: BandwidthProfile[]) {
  return mountComponent(BandwidthProfiles, { messages: { bandwidth }, props: { modelValue: profiles } })
}

/** The summary line under a profile's name. */
function summaryOf(name: string): string {
  return screen.getByText(name).nextElementSibling?.textContent?.trim() ?? ''
}

describe('the bandwidth profile list', () => {
  it('names a monthly budget instead of reading as unlimited', () => {
    mount([profile({ id: 'cap', name: 'Monthly cap 1 TB', monthly_budget_bytes: String(1024 ** 4) })])

    expect(summaryOf('Monthly cap 1 TB')).toBe('Unlimited · 1.0 TiB per month · 0 scope limits')
  })

  it('names the parallel-file cap, the daily budget and the upload limit as well', () => {
    mount([
      profile({ id: 'night', name: 'Night', max_active_files: 6 }),
      profile({
        id: 'office',
        name: 'Office',
        download_bytes_per_second: String(4 * 1024 ** 2),
        upload_bytes_per_second: String(1024 ** 2),
        max_active_files: 1,
        daily_budget_bytes: String(50 * 1024 ** 3),
        scopes: [{ kind: 'protocol', value: 'usenet', bytes_per_second: 1024 ** 2 }] as BandwidthProfile['scopes']
      })
    ])

    expect(summaryOf('Night')).toBe('Unlimited · 6 parallel files · 0 scope limits')
    expect(summaryOf('Office')).toBe('4.0 MiB/s · 1.0 MiB/s upload · 1 parallel file · 50.0 GiB per day · 1 scope limit')
  })
})
