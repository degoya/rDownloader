/**
 * An emptied override inherits again (RD-1110-10, RD-1120-09).
 *
 * The number field hands an emptied field `undefined`; the request says "inherit" with `null`,
 * and a body without the key would mean something else to an endpoint that keeps what it is not
 * sent.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { SeedingPolicyResponse } from '@/api/types'
import torrent from '@/locales/en/torrent.json'
import { mountComponent } from '@/test/mount'

import TorrentSeedingPolicy from './TorrentSeedingPolicy.vue'

const policy: SeedingPolicyResponse = {
  effective: {
    enabled: true,
    enabled_source: 'torrent',
    ratio: 1.5,
    ratio_source: 'torrent',
    time: { minutes: 30 },
    time_source: 'torrent'
  },
  torrent_override: { enabled: true, ratio_milli: 1500, time: { minutes: 30 } }
}

async function clearAndSave(label: string) {
  const view = mountComponent(TorrentSeedingPolicy, { messages: { torrent }, props: { policy } })
  const field = screen.getByLabelText(label) as HTMLInputElement
  await fireEvent.update(field, '')
  await fireEvent.submit(field.closest('form') as HTMLFormElement)
  return view.emitted<[Record<string, unknown>]>().save?.at(-1)?.[0]
}

describe('the seeding override', () => {
  it('sends an emptied ratio as null, so the torrent inherits it', async () => {
    expect(await clearAndSave(torrent.seeding.override_ratio)).toMatchObject({ ratio: null, time_minutes: 30 })
  })

  it('sends an emptied seed time as null, so the torrent inherits it', async () => {
    expect(await clearAndSave(torrent.seeding.override_time)).toMatchObject({ ratio: 1.5, time_minutes: null })
  })
})
