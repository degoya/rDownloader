/**
 * The one copy of the enricher chips (RD-150-19): the queue and the LinkGrabber each rendered
 * them, printing the raw English suffix of the field name in every language.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { EnrichmentField } from '@/api/types'
import deCommon from '@/locales/de/common.json'
import { mountComponent } from '@/test/mount'

import EnrichmentChips from './EnrichmentChips.vue'

function field(name: string, value: string): EnrichmentField {
  return { name, value, plugin_id: 'metadata-enricher', fetched_at: '2026-09-27T10:00:00Z' }
}

describe('EnrichmentChips', () => {
  it('translates the fields the interface knows', () => {
    mountComponent(EnrichmentChips, {
      locale: 'de',
      messages: { common: deCommon },
      props: { fields: [field('metadata.season', '2'), field('metadata.runtime', '47 min'), field('metadata.rating', '9.5')] }
    })
    expect(screen.getByText('Staffel: 2')).toBeTruthy()
    expect(screen.getByText('Laufzeit: 47 min')).toBeTruthy()
    expect(screen.getByText('Bewertung: 9.5')).toBeTruthy()
  })

  it('falls back to the raw suffix for a field it does not know', () => {
    mountComponent(EnrichmentChips, { props: { fields: [field('sponsorblock.sponsor_seconds', '42')] } })
    expect(screen.getByText('sponsor_seconds: 42')).toBeTruthy()
  })

  it('names the full field and its lookup time in the title', () => {
    mountComponent(EnrichmentChips, { props: { fields: [field('metadata.year', '2008')] } })
    const chip = screen.getByText('Year: 2008')
    expect(chip.getAttribute('title')).toContain('metadata.year, looked up by a plugin on')
  })

  it('renders nothing for no fields', () => {
    const { container } = mountComponent(EnrichmentChips, { props: { fields: [] } })
    expect(container.querySelectorAll('[data-testid="enrichment-chip"]')).toHaveLength(0)
  })
})
