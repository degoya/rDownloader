import { describe, expect, it } from 'vitest'

import type { CandidateSource } from '@/api/types'
import common from '@/locales/en/common.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import CollectorCandidateSources from './CollectorCandidateSources.vue'

function source(index: number, overrides: Partial<CandidateSource> = {}): CandidateSource {
  return {
    url: `https://mirror${index}.example/image.iso`,
    host: `mirror${index}.example`,
    protocol: 'https',
    priority: null,
    location: null,
    ...overrides
  }
}

function mount(sources: CandidateSource[]) {
  return mountComponent(CollectorCandidateSources, {
    props: { sources },
    messages: { common, linkgrabber }
  })
}

describe('CollectorCandidateSources', () => {
  it('lists a link’s mirrors before queueing, in the order the transfer tries them', () => {
    const { container } = mount([
      source(0, { priority: 1, location: 'de' }),
      source(1, { url: 'https://mirror1.example/image.iso?token=***' }),
      source(2, { url: 'ftp://mirror2.example/image.iso', protocol: 'ftp' })
    ])
    const rows = [...container.querySelectorAll('[data-testid="candidate-source"]')]
    expect(rows.map(row => row.querySelector('.font-mono')?.textContent)).toEqual([
      'https://mirror0.example/image.iso',
      'https://mirror1.example/image.iso?token=***',
      'ftp://mirror2.example/image.iso'
    ])
    expect(container.textContent).toContain('Sources (3)')
    expect(rows[0]?.textContent).toContain('Priority 1')
    expect(rows[0]?.textContent).toContain('de')
    expect(rows[2]?.querySelector('[label="ftp"]')).not.toBeNull()
  })

  it('says which addresses the queue will not request', () => {
    const { container } = mount([source(0)])
    expect(container.textContent).toContain('An address that points at your own machine')
  })

  it('renders nothing for a link without mirrors', () => {
    const { container } = mount([])
    expect(container.querySelector('[data-testid="candidate-sources"]')).toBeNull()
  })
})
