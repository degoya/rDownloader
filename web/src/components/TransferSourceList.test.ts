import { describe, expect, it } from 'vitest'

import type { DownloadSourcesResponse, DownloadSourceView } from '@/api/types'
import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'

import TransferSourceList from './TransferSourceList.vue'

function source(position: number, overrides: Partial<DownloadSourceView> = {}): DownloadSourceView {
  return {
    position,
    url: `https://mirror${position}.example/file.iso`,
    host: `mirror${position}.example`,
    protocol: 'https',
    priority: position + 1,
    location: null,
    state: 'ready',
    failures: 0,
    backoff_until: null,
    isolated_code: null,
    last_error_code: null,
    delivered_bytes: 0,
    ...overrides
  }
}

function mount(sources: DownloadSourcesResponse) {
  return mountComponent(TransferSourceList, {
    props: { sources },
    messages: { common, downloads }
  })
}

describe('TransferSourceList', () => {
  it('lists every mirror in the order the queue tries them', () => {
    const { container } = mount({
      sources: [source(0, { location: 'de' }), source(1), source(2)],
      piece_hashes: { algorithm: 'sha1', piece_length: 262144, pieces: 40 }
    })
    const urls = [...container.querySelectorAll('li .font-mono')].map(node => node.textContent)
    expect(urls).toEqual([
      'https://mirror0.example/file.iso',
      'https://mirror1.example/file.iso',
      'https://mirror2.example/file.iso'
    ])
    expect(container.textContent).toContain('Sources (3)')
    expect(container.textContent).toContain('Priority 1')
    expect(container.textContent).toContain('Checked in 40 pieces')
  })

  it('says why a mirror is excluded and when a waiting one is tried again', () => {
    const { container } = mount({
      sources: [
        source(0, { state: 'isolated', isolated_code: 'mirror.piece_hash_mismatch' }),
        source(1, { state: 'backing_off', failures: 2, backoff_until: '2026-09-27T12:00:00Z' })
      ],
      piece_hashes: null
    })
    expect(container.querySelector('[data-state="isolated"] [label="Excluded"]')).not.toBeNull()
    expect(container.textContent).toContain('Delivered data that did not match its hash.')
    expect(container.textContent).toContain('2 failures')
    expect(container.textContent).toContain('again from')
    expect(container.textContent).toContain('No piece hashes')
  })

  it('names FTP and SFTP mirrors as kept but not yet used', () => {
    const { container } = mount({
      sources: [source(0), source(1, { protocol: 'ftp', state: 'unsupported' })],
      piece_hashes: null
    })
    expect(container.querySelector('[data-state="unsupported"] [label="Not used yet"]')).not.toBeNull()
    expect(container.textContent).toContain('FTP and SFTP sources are shown but not yet used for chunks.')
  })

  it('renders nothing for a download with a single address', () => {
    const { container } = mount({ sources: [], piece_hashes: null })
    expect(container.textContent?.trim()).toBe('')
  })
})
