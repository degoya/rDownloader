import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { EmbedWarning, MediaEmbedPolicy } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import MediaEmbedPolicyCard from './MediaEmbedPolicyCard.vue'

/**
 * The checkbox as one element carrying the field's test id and `disabled`; the shared stub
 * repeats its attributes on the label and the input.
 */
const UCheckbox = {
  props: ['modelValue', 'label'],
  emits: ['update:modelValue'],
  template: '<button type="button" v-bind="$attrs" @click="$emit(\'update:modelValue\', modelValue !== true)">{{ label }}</button>'
}

function policy(overrides: Partial<MediaEmbedPolicy> = {}): MediaEmbedPolicy {
  return {
    thumbnail: false,
    chapters: false,
    metadata: false,
    info_json: false,
    sponsorblock: { mode: 'off', categories: [] },
    ...overrides
  } as MediaEmbedPolicy
}

function mount(props: { embed?: MediaEmbedPolicy, warnings?: EmbedWarning[], canTranscode?: boolean }) {
  return mountComponent(MediaEmbedPolicyCard, {
    messages: { linkgrabber: en },
    props: {
      embed: props.embed ?? policy(),
      warnings: props.warnings ?? [],
      canTranscode: props.canTranscode ?? true
    },
    stubs: { UCheckbox }
  })
}

describe('MediaEmbedPolicyCard', () => {
  it('starts with everything off, including SponsorBlock', () => {
    mount({})
    // Nothing is written into the file until it is asked for; embedding cannot be undone
    // without re-downloading.
    expect(screen.getByTestId('media-embed-thumbnail')).toBeTruthy()
    expect(screen.queryByTestId('media-sponsor-categories')).toBeNull()
    expect(screen.queryByTestId('media-sponsor-destructive')).toBeNull()
  })

  it('warns that removing segments re-cuts the media, and marking does not', async () => {
    const { rerender } = mount({ embed: policy({ sponsorblock: { mode: 'mark', categories: [] } }) })
    expect(screen.queryByTestId('media-sponsor-destructive')).toBeNull()
    expect(screen.getByTestId('media-sponsor-categories')).toBeTruthy()

    await rerender({ embed: policy({ sponsorblock: { mode: 'remove', categories: [] } }) })
    expect(screen.getByTestId('media-sponsor-destructive').textContent).toContain('re-cuts the media')
  })

  it('emits the whole policy when one piece is toggled', async () => {
    const { emitted } = mount({})
    await fireEvent.click(screen.getByTestId('media-embed-chapters'))
    const change = emitted().change as MediaEmbedPolicy[][]
    const [next] = change[0] ?? []
    expect(next?.chapters).toBe(true)
    expect(next?.thumbnail).toBe(false)
  })

  it('disables every piece when ffmpeg is unavailable', () => {
    mount({ canTranscode: false })
    expect(screen.getByTestId('media-embed-metadata').hasAttribute('disabled')).toBe(true)
    expect((screen.getByRole('radio', { name: 'Remove' }) as HTMLInputElement).disabled).toBe(true)
  })

  // RD-1120-14: the mode is a radio group, so the chosen one is announced, not only coloured.
  it('offers the SponsorBlock mode as radios with the chosen one checked', async () => {
    const { emitted } = mount({ embed: policy({ sponsorblock: { mode: 'mark', categories: [] } }) })
    expect((screen.getByRole('radio', { name: 'Mark as chapters' }) as HTMLInputElement).checked).toBe(true)
    expect((screen.getByRole('radio', { name: 'Remove' }) as HTMLInputElement).checked).toBe(false)
    await fireEvent.click(screen.getByRole('radio', { name: 'Remove' }))
    const [next] = (emitted().change as MediaEmbedPolicy[][])[0] ?? []
    expect(next?.sponsorblock.mode).toBe('remove')
  })

  it('says when a signed source URL is withheld from the file', () => {
    mount({
      embed: policy({ info_json: true }),
      warnings: [{ kind: 'source_url_withheld' } as EmbedWarning]
    })
    expect(screen.getByTestId('media-embed-warning').textContent).toContain('signature or token')
  })

  it('reports a container that cannot hold what was asked for', () => {
    mount({
      embed: policy({ thumbnail: true }),
      warnings: [{ kind: 'thumbnail_unsupported', container: 'webm' } as EmbedWarning]
    })
    expect(screen.getByTestId('media-embed-warning').textContent).toContain('WEBM file cannot hold a cover image')
  })
})
