/**
 * RD-1390-07: "Reset the package completely" in a package's menu — offered for a failed package
 * only, and handed to the view, whose dialog confirms it.
 */
import { screen, within } from '@testing-library/vue'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { toastMock } from '@/test/mocks'

import type { DownloadPackage } from '@/api/types'
import { i18n } from '@/i18n'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent, passthrough } from '@/test/mount'

import PackageGroup from './PackageGroup.vue'

vi.mock('@nuxt/ui/composables', () => toastMock())

interface MenuItem { label: string, onSelect?: () => void }

/** The dropdown rendered open, its items real buttons. */
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

const file = (id: string, state: string) => ({ id, file_name: `${id}.rar`, state, committed_bytes: '0', total_bytes: '1', kind: 'http' })

function renderGroup(value: DownloadPackage, files: unknown[]) {
  return mountComponent(PackageGroup, {
    messages: { downloads, common },
    props: {
      package: value,
      downloads: files,
      categories: [],
      selection: 'none',
      open: false,
      complete: false,
      packageRate: 0,
      packageEta: null,
      dragging: false,
      canPause: false,
      canResume: false,
      controlBusy: null
    },
    stubs: { UDropdownMenu, UBadge: passthrough }
  })
}

beforeAll(() => {
  i18n.global.locale.value = 'en'
})

describe('PackageGroup full reset', () => {
  it('offers the full reset for a package with a failed file and hands it to the view', async () => {
    const { emitted } = renderGroup(group(), [file('a', 'failed'), file('b', 'completed')])
    const cell = document.querySelector('.queue-cell-actions') as HTMLElement
    ;(within(cell).getByText(downloads.package.reset_all) as HTMLButtonElement).click()
    await Promise.resolve()
    expect(emitted().resetPackage?.[0]).toEqual(['package-1'])
  })

  it('offers it for a package whose post-processing failed, every file finished', () => {
    renderGroup(group({ state: 'failed', extraction_result: 'failed' } as unknown as Partial<DownloadPackage>), [file('a', 'completed')])
    expect(screen.getAllByText(downloads.package.reset_all).length).toBeGreaterThan(0)
  })

  it('offers nothing of the kind for a package that did not fail', () => {
    renderGroup(group(), [file('a', 'completed'), file('b', 'queued')])
    expect(screen.queryByText(downloads.package.reset_all)).toBeNull()
  })
})
