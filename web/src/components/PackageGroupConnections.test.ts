/**
 * RD-1270-21: a Usenet package counts the connections its files hold, not the files.
 *
 * "2 active" beside two files that together kept all ten of a server's connections busy said
 * nothing of the other eight. The glyph and the count take the place of the words, at the same
 * width; HTTP packages keep counting their running files.
 */
import { screen } from '@testing-library/vue'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { toastMock } from '@/test/mocks'

import type { Download, DownloadPackage } from '@/api/types'
import { i18n } from '@/i18n'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent, passthrough } from '@/test/mount'

import PackageGroup from './PackageGroup.vue'

vi.mock('@nuxt/ui/composables', () => toastMock())

function render(kind: 'usenet' | 'http', connections: unknown) {
  const files = ['a', 'b', 'c'].map(id => ({
    id, state: id === 'c' ? 'queued' : 'downloading', file_name: `${id}.rar`,
    committed_bytes: '0', total_bytes: '100', kind
  })) as unknown as Download[]
  return mountComponent(PackageGroup, {
    messages: { downloads, common },
    props: {
      package: {
        id: 'package-1', name: 'Some Release', state: 'downloading', destination: '/downloads',
        category_id: null, priority: 'normal', position: 1, has_password: false, kind,
        nzb_import_id: kind === 'usenet' ? 'import-1' : null, enrichment: []
      } as unknown as DownloadPackage,
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
      controlBusy: null,
      connections
    },
    stubs: { UDropdownMenu: passthrough, UBadge: passthrough }
  })
}

beforeAll(() => {
  i18n.global.locale.value = 'en'
})

describe('PackageGroup connections', () => {
  it('shows the busy connections with their split in place of the running files', () => {
    render('usenet', { busy: 10, perFile: [5, 5], serverBusy: 10, serverLimit: 10 })
    const glyph = screen.getByTestId('package-connections')
    expect(glyph.textContent?.trim()).toBe('10')
    expect(glyph.getAttribute('aria-label')).toBe('10 connections: 2 files at once, 5 each. All 10 connections the servers allow are busy.')
    expect(screen.queryByText(/2 active/)).toBeNull()
  })

  it('keeps counting running files while no connection is busy', () => {
    render('usenet', null)
    expect(screen.queryByTestId('package-connections')).toBeNull()
    expect(screen.getByText(/2 active/)).toBeTruthy()
  })

  it('leaves an HTTP package as it was, whatever it is given', () => {
    render('http', { busy: 4, perFile: [4], serverBusy: 4, serverLimit: 10 })
    expect(screen.queryByTestId('package-connections')).toBeNull()
    expect(screen.getByText(/2 active/)).toBeTruthy()
  })
})
