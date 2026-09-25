/**
 * What the package header spends the row's width on.
 *
 * RD-109-30: the header had grown to a password, two spelled-out badges, a spelled-out
 * priority and seven loose icon buttons, and the one thing it could not show was the package
 * name. Every saving below is a thing that used to take width; each is asserted by the name it
 * kept rather than by the glyph that replaced it, because an icon that lost its accessible
 * name is a regression that looks like a success.
 */
import { screen, within } from '@testing-library/vue'
import { beforeAll, describe, expect, it } from 'vitest'

import type { DownloadPackage } from '@/api/types'
import { i18n } from '@/i18n'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent, passthrough } from '@/test/mount'

import PackageGroup from './PackageGroup.vue'

interface MenuItem { label: string, onSelect?: () => void }

/**
 * The dropdown rendered open: its trigger stays in place and its items become real buttons,
 * inside a marker so a test can tell "beside the row" from "under the dots".
 */
const UDropdownMenu = {
  props: ['items'],
  methods: {
    run(item: MenuItem) { item.onSelect?.() }
  },
  template: `<div><slot />
    <div data-menu-items>
      <button v-for="item in (items ?? []).flat()" :key="item.label" type="button" @click="run(item)">{{ item.label }}</button>
    </div>
  </div>`
}

function group(overrides: Partial<DownloadPackage> = {}): DownloadPackage {
  return {
    id: 'package-1',
    name: 'Some Release',
    state: 'queued',
    destination: '/downloads/Some Release',
    category_id: null,
    priority: 'normal',
    position: 1,
    has_password: false,
    kind: 'http',
    nzb_import_id: null,
    created_at: '2026-09-02T10:00:00Z',
    enrichment: [],
    ...overrides
  } as unknown as DownloadPackage
}

function renderGroup(value: DownloadPackage, props: Record<string, unknown> = {}) {
  return mountComponent(PackageGroup, {
    messages: { downloads, common },
    props: {
      package: value,
      downloads: [],
      categories: [],
      selection: 'none',
      open: false,
      complete: false,
      packageRate: 0,
      packageEta: null,
      dragging: false,
      canPause: false,
      canResume: false,
      controlBusy: null,
      ...props
    },
    // `UBadge` has to keep its attributes for the icon badges to be findable by their name.
    stubs: { UDropdownMenu, UBadge: passthrough }
  })
}

beforeAll(() => {
  // `priorityItems()` reads the application's own i18n, not the test's.
  i18n.global.locale.value = 'en'
})

describe('PackageGroup enrichment', () => {
  // RD-107-02: what an enricher found used to be visible only while the link sat in the
  // LinkGrabber. The download list now shows it with the same chips the candidate row uses.
  it('shows the enricher fields as chips naming their plugin', () => {
    renderGroup(group({
      enrichment: [
        { name: 'imdb.score', value: '9.3', plugin_id: 'imdb-enricher', fetched_at: '2026-09-02T10:05:00Z' }
      ]
    } as unknown as Partial<DownloadPackage>))
    const chip = screen.getByText(/score: 9\.3/)
    expect(chip).toBeTruthy()
    expect(chip.getAttribute('title')).toContain('imdb.score')
  })

  it('shows no chip row for a package nothing enriched', () => {
    renderGroup(group())
    expect(screen.queryByText(/score:/)).toBeNull()
  })
})

describe('PackageGroup password', () => {
  const password = 'Ab3-kX9!qR7_mZ2@vT5#wL8$dN4%'

  const done = [{ id: 'd1', file_name: 'release.rar', state: 'completed', committed_bytes: '100', total_bytes: '100' }]

  /** Once the archives are out it opens nothing, and it was taking the name's width. */
  it('drops the password once the download is done and the archives have been extracted', () => {
    renderGroup(
      group({ has_password: true, password, extraction_result: 'success' } as unknown as Partial<DownloadPackage>),
      { complete: true, downloads: done }
    )
    expect(screen.queryByText(password)).toBeNull()
    expect(screen.queryByTitle(downloads.package.password_stored)).toBeNull()
  })

  /**
   * `extraction_result` is the outcome of the *last* pipeline run and survives completion, so a
   * package unpacked once keeps reporting `success` while files added to it afterwards are still
   * coming down. That is exactly when the password is needed again, so the unpack alone must not
   * hide it — the download has to be finished too.
   */
  it('keeps the password while a later file is still downloading into an unpacked package', () => {
    renderGroup(
      group({ has_password: true, password, extraction_result: 'success' } as unknown as Partial<DownloadPackage>),
      { downloads: [
        ...done,
        { id: 'd2', file_name: 'extra.rar', state: 'downloading', committed_bytes: '40', total_bytes: '100' }
      ] }
    )
    expect(screen.getByText(password)).toBeTruthy()
  })

  it('keeps the password while a later file is still queued, not only while it runs', () => {
    renderGroup(
      group({ has_password: true, password, extraction_result: 'success' } as unknown as Partial<DownloadPackage>),
      { downloads: [
        ...done,
        { id: 'd2', file_name: 'extra.rar', state: 'queued', committed_bytes: '0', total_bytes: '100' }
      ] }
    )
    expect(screen.getByText(password)).toBeTruthy()
  })

  it('keeps the password while the unpack is still outstanding', () => {
    renderGroup(group({ has_password: true, password } as unknown as Partial<DownloadPackage>))
    expect(screen.getByText(password)).toBeTruthy()
  })

  it('keeps the password when the unpack failed, which is when it is read', () => {
    renderGroup(group({
      has_password: true,
      password,
      state: 'failed',
      extraction_result: 'failed'
    } as unknown as Partial<DownloadPackage>))
    expect(screen.getByText(password)).toBeTruthy()
  })
})

describe('PackageGroup state badges', () => {
  it('names the finished badge although it shows only an icon', () => {
    renderGroup(group(), { complete: true })
    const badge = screen.getByLabelText(downloads.package.complete)
    expect(badge).toBeTruthy()
    // The word is the name, not the content: that is the width the saving bought.
    expect(badge.textContent?.trim()).toBe('')
    expect(badge.getAttribute('title')).toBe(downloads.package.complete_title)
  })

  it('names the extracted badge although it shows only an icon', () => {
    renderGroup(
      group({ extraction_result: 'success' } as unknown as Partial<DownloadPackage>),
      { complete: true }
    )
    const badge = screen.getByLabelText(downloads.package.extracted)
    expect(badge.textContent?.trim()).toBe('')
    expect(badge.getAttribute('title')).toBe(downloads.package.extracted_title)
  })
})

describe('PackageGroup priority', () => {
  it.each([
    ['high', common.priority.high],
    ['normal', common.priority.normal],
    ['low', common.priority.low]
  ])('names the %s level on the icon control', (priority, level) => {
    renderGroup(group({ priority } as unknown as Partial<DownloadPackage>))
    const control = screen.getByLabelText(`Priority: ${level}`)
    expect(control).toBeTruthy()
    expect(control.textContent?.trim()).toBe('')
  })

  it('offers every level by name under that control', async () => {
    const { emitted } = renderGroup(group({ priority: 'normal' } as unknown as Partial<DownloadPackage>))
    const meta = document.querySelector('.queue-cell-meta') as HTMLElement
    for (const level of [common.priority.high, common.priority.normal, common.priority.low]) {
      expect(within(meta).getByText(level)).toBeTruthy()
    }
    ;(within(meta).getByText(common.priority.high) as HTMLButtonElement).click()
    await Promise.resolve()
    expect(emitted().priority?.[0]).toEqual(['package-1', 'high'])
  })

  it('offers no priority once the package is finished', () => {
    renderGroup(group(), { complete: true })
    expect(screen.queryByLabelText(`Priority: ${common.priority.normal}`)).toBeNull()
  })
})

describe('PackageGroup row actions', () => {
  /** Everything in the last cell that is not an item of the dots menu. */
  function controlsBesideTheRow(): HTMLElement[] {
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    return [...cell.querySelectorAll('button')].filter(button => !button.closest('[data-menu-items]'))
  }

  function menuItems(): string[] {
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    const list = cell.querySelector('[data-menu-items]') as HTMLElement
    return [...list.querySelectorAll('button')].map(button => button.textContent?.trim() ?? '')
  }

  it('keeps at most two controls beside the row', () => {
    renderGroup(group({ kind: 'usenet', nzb_import_id: 'import-1' } as unknown as Partial<DownloadPackage>), { canPause: true })
    expect(controlsBesideTheRow()).toHaveLength(2)
  })

  it('keeps only the dots when the package has nothing to start or stop', () => {
    renderGroup(group(), { complete: true })
    expect(controlsBesideTheRow()).toHaveLength(1)
  })

  it('moves the remaining actions under the dots with their labels intact', () => {
    renderGroup(
      group({ kind: 'usenet', nzb_import_id: 'import-1' } as unknown as Partial<DownloadPackage>),
      { complete: true, downloads: [{ id: 'd1', file_name: 'release.rar', state: 'completed', committed_bytes: '100', total_bytes: '100' }] }
    )
    const labels = menuItems()
    for (const label of [
      downloads.package.copy_path_aria,
      common.actions.extract,
      downloads.package.segments_aria,
      downloads.package.postprocess_aria,
      downloads.package.edit_aria,
      downloads.package.delete_aria
    ]) {
      expect(labels).toContain(label)
    }
  })

  it('still reaches the package through the menu it moved into', async () => {
    const { emitted } = renderGroup(group(), { complete: true })
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    ;(within(cell).getByText(downloads.package.delete_aria) as HTMLButtonElement).click()
    await Promise.resolve()
    expect(emitted().deletePackage?.[0]).toEqual(['package-1'])
  })
})

describe('PackageGroup progress', () => {
  it('drops the percentage beside a full bar, which says the same thing', () => {
    renderGroup(group(), {
      complete: true,
      downloads: [{ id: 'd1', file_name: 'a.bin', state: 'completed', committed_bytes: '100', total_bytes: '100' }]
    })
    const progress = document.querySelector('.queue-cell-progress') as HTMLElement
    expect(progress.textContent?.trim()).toBe('')
  })

  it('keeps the percentage while the bar is short of the end', () => {
    renderGroup(group(), {
      downloads: [{ id: 'd1', file_name: 'a.bin', state: 'downloading', committed_bytes: '40', total_bytes: '100' }]
    })
    const progress = document.querySelector('.queue-cell-progress') as HTMLElement
    expect(progress.textContent?.trim()).toBe('40%')
  })
})

/**
 * The shared grid places nine named cells rather than trusting source order, so a row that
 * loses one silently stops wrapping and starts overlapping instead (`web/src/assets/main.css`).
 */
describe('PackageGroup grid cells', () => {
  it('carries every named cell of the shared queue row', () => {
    renderGroup(group())
    const row = document.querySelector('.queue-row') as HTMLElement
    for (const cell of ['handle', 'select', 'expand', 'name', 'state', 'progress', 'size', 'meta', 'actions']) {
      expect(row.querySelector(`.queue-cell-${cell}`), cell).toBeTruthy()
    }
  })
})
