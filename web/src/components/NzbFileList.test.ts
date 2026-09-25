/**
 * What the segment panel is allowed to say (RD-109-30).
 *
 * The panel opens directly above the package's own download rows, which carry the same file
 * names. Reported with a screenshot: six panel lines and, one line below, six rows repeating
 * them. So the rule is that the panel may only carry what those rows cannot — the raw NZB
 * subject with the poster's ordering, the segment tally, and the segments that never arrived.
 *
 * The size is the case worth a test rather than a comment. `NzbFileStatus.total_bytes` sums
 * the `<segment bytes>` attributes, which is the posted article size *including* the yEnc
 * overhead, while the download row shows the decoded file — so the same file read "722 MiB"
 * here and "699 MiB" underneath. That is not a duplicate, it is a contradiction, and it is
 * what must not come back.
 */
import { screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { NzbFileStatus } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'

const files: NzbFileStatus[] = [{
  id: 'file-1',
  import_id: 'import-1',
  // 722 MiB of posted articles; the download row below shows the 699 MiB that come out.
  subject: '[6/6] "Adventure.Time.S01E01.H.265-FUZEER.mkv" yEnc (1/1023)',
  poster: 'poster@example.invalid',
  groups: ['alt.binaries.example'],
  total_bytes: '757100544',
  ordinal: 6,
  output_path: null,
  assembly_name: null,
  declared_size: null,
  segments: [
    ...Array.from({ length: 3 }, (_, index) => ({ id: `s${index}`, number: index + 1, bytes: '739 ', message_id: `m${index}`, state: 'completed', server_attempts: 1, crc32: null, part_begin: null, part_end: null })),
    { id: 's3', number: 4, bytes: '739', message_id: 'm3', state: 'failed', server_attempts: 3, crc32: null, part_begin: null, part_end: null }
  ]
}] as unknown as NzbFileStatus[]

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: files })) },
  responseError: () => 'failed'
}))

const { default: NzbFileList } = await import('./NzbFileList.vue')

function renderList() {
  return mountComponent(NzbFileList, { messages: { downloads }, props: { importId: 'import-1' } })
}

describe('NzbFileList', () => {
  it('shows the segment tally and the segments that never arrived', async () => {
    renderList()
    await waitFor(() => expect(screen.getByText(/3\/4/)).toBeTruthy())
    expect(screen.getByText(new RegExp(downloads.nzb.missing.replace('{count}', '1')))).toBeTruthy()
  })

  it('shows the raw NZB subject, which the download row does not have', async () => {
    renderList()
    await waitFor(() => expect(screen.getByText(/\[6\/6\]/)).toBeTruthy())
    expect(screen.getByText(/yEnc \(1\/1023\)/)).toBeTruthy()
  })

  /** The row underneath already carries a size, and it is a different quantity than this one. */
  it('repeats no size from the download rows underneath it', async () => {
    renderList()
    await waitFor(() => expect(screen.getByText(/3\/4/)).toBeTruthy())
    expect(document.body.textContent).not.toMatch(/\d+(\.\d+)?\s?(B|KiB|MiB|GiB|TiB)\b/)
  })
})
