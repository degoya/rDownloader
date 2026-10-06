import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { AudioTrack, SubtitleTrack, TrackSelection, TrackWarning } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import MediaTrackSelector from './MediaTrackSelector.vue'

/**
 * The checkbox as one element carrying the field's test id and its description; the shared stub
 * repeats its attributes on the label and the input.
 */
const UCheckbox = {
  props: ['modelValue', 'label', 'description'],
  emits: ['update:modelValue'],
  template: '<button type="button" v-bind="$attrs" @click="$emit(\'update:modelValue\', modelValue !== true)">{{ label }} {{ description }}</button>'
}

function tracks(overrides: Partial<TrackSelection> = {}): TrackSelection {
  return {
    audio: { extra_languages: [] },
    subtitles: { mode: 'off', languages: [], include_automatic: false, convert_to: null },
    ...overrides
  } as TrackSelection
}

function mount(props: {
  tracks?: TrackSelection
  audioTracks?: AudioTrack[]
  subtitles?: SubtitleTrack[]
  warnings?: TrackWarning[]
  canMerge?: boolean
}) {
  return mountComponent(MediaTrackSelector, {
    messages: { linkgrabber: en },
    stubs: { UCheckbox },
    props: {
      tracks: props.tracks ?? tracks(),
      audioTracks: props.audioTracks ?? [],
      subtitles: props.subtitles ?? [],
      warnings: props.warnings ?? [],
      canMerge: props.canMerge ?? true
    }
  })
}

const manual = (language: string): SubtitleTrack =>
  ({ language, name: null, source: 'manual', formats: ['vtt'] }) as SubtitleTrack
const automatic = (language: string): SubtitleTrack =>
  ({ language, name: null, source: 'automatic', formats: ['vtt'] }) as SubtitleTrack

describe('MediaTrackSelector', () => {
  it('says when a language exists only as an auto-generated track', () => {
    // Silently using it would put a speech-recognition guess in the file under the same
    // name as an authored translation.
    mount({ tracks: tracks({ subtitles: { mode: 'embed', languages: [], include_automatic: false, convert_to: null } }), subtitles: [automatic('en')] })
    expect(screen.getByTestId('media-automatic-only').textContent).toContain('only auto-generated subtitles')
  })

  it('does not warn once auto-generated subtitles are explicitly enabled', () => {
    mount({ tracks: tracks({ subtitles: { mode: 'embed', languages: [], include_automatic: true, convert_to: null } }), subtitles: [automatic('en')] })
    expect(screen.queryByTestId('media-automatic-only')).toBeNull()
  })

  it('hides the language and conversion fields while subtitles are off', () => {
    mount({ subtitles: [manual('de')] })
    expect(screen.queryByTestId('media-subtitle-languages')).toBeNull()
    expect(screen.queryByTestId('media-include-automatic')).toBeNull()
  })

  it('disables embedding when ffmpeg is unavailable', () => {
    mount({ canMerge: false, subtitles: [manual('de')] })
    const embed = screen.getByRole('radio', { name: 'Embedded' }) as HTMLInputElement
    expect(embed.disabled).toBe(true)
    expect(screen.getByText('Embedded').getAttribute('title')).toBe(en.media.tracks.embed_needs_ffmpeg)
    // A sidecar file needs no remux, so it stays offered.
    const sidecar = screen.getByRole('radio', { name: 'Separate file' }) as HTMLInputElement
    expect(sidecar.disabled).toBe(false)
  })

  // RD-1120-14: the mode is a radio group, so the chosen one is announced, not only coloured.
  it('checks the radio of the subtitle mode in force', () => {
    mount({ tracks: tracks({ subtitles: { mode: 'embed', languages: [], include_automatic: false, convert_to: null } }), subtitles: [manual('de')] })
    expect((screen.getByRole('radio', { name: 'Embedded' }) as HTMLInputElement).checked).toBe(true)
    expect((screen.getByRole('radio', { name: 'Separate file' }) as HTMLInputElement).checked).toBe(false)
  })

  it('emits the whole selection when the subtitle mode changes', async () => {
    const { emitted } = mount({ subtitles: [manual('de')] })
    await fireEvent.click(screen.getByRole('radio', { name: 'Separate file' }))
    const change = emitted().change as TrackSelection[][]
    const [selection] = change[0] ?? []
    expect(selection?.subtitles.mode).toBe('sidecar')
    expect(selection?.audio.extra_languages).toEqual([])
  })

  it('renders the container limits reported by the backend', () => {
    mount({
      warnings: [
        { kind: 'multiple_audio_unsupported', container: 'mp3' } as TrackWarning,
        { kind: 'only_automatic_available', language: 'de' } as TrackWarning
      ]
    })
    const warnings = screen.getAllByTestId('media-track-warning')
    expect(warnings[0]?.textContent).toContain('MP3 file cannot hold more than one audio track')
    expect(warnings[1]?.textContent).toContain('auto-generated DE track')
  })
})
