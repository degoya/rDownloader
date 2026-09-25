import { fireEvent, render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { AudioTrack, SubtitleTrack, TrackSelection, TrackWarning } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'

import MediaTrackSelector from './MediaTrackSelector.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UAlert: { props: ['description'], template: '<div v-bind="$attrs">{{ description }}<slot /></div>' },
  UButton: { props: ['label'], template: '<button v-bind="$attrs">{{ label }}<slot /></button>' },
  UCheckbox: {
    props: ['modelValue', 'label', 'description'],
    emits: ['update:modelValue'],
    template: '<button type="button" v-bind="$attrs" @click="$emit(\'update:modelValue\', modelValue !== true)">{{ label }} {{ description }}</button>'
  },
  UFormField: { props: ['label'], template: '<label v-bind="$attrs">{{ label }}<slot /></label>' },
  UIcon: passthrough,
  USelect: { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"><slot /></select>' },
  USelectMenu: { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"><slot /></select>' }
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
  return render(MediaTrackSelector, {
    props: {
      tracks: props.tracks ?? tracks(),
      audioTracks: props.audioTracks ?? [],
      subtitles: props.subtitles ?? [],
      warnings: props.warnings ?? [],
      canMerge: props.canMerge ?? true
    },
    global: { plugins: [i18n], components }
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
    const embed = screen.getByText('Embedded').closest('button')
    expect(embed?.hasAttribute('disabled')).toBe(true)
    // A sidecar file needs no remux, so it stays offered.
    const sidecar = screen.getByText('Separate file').closest('button')
    expect(sidecar?.hasAttribute('disabled')).toBe(false)
  })

  it('emits the whole selection when the subtitle mode changes', async () => {
    const { emitted } = mount({ subtitles: [manual('de')] })
    await fireEvent.click(screen.getByText('Separate file'))
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
