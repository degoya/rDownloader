import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { MediaFormatsResponse } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'

import MediaFormatSelector from './MediaFormatSelector.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en } } })

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UAlert: {
    props: ['title', 'description'],
    template: '<div v-bind="$attrs"><span>{{ title }}</span><span>{{ description }}</span><slot name="description" /></div>'
  },
  UBadge: passthrough,
  UButton: {
    props: ['label'],
    template: '<button v-bind="$attrs">{{ label }}<slot /></button>'
  },
  UFormField: { props: ['label'], template: '<label v-bind="$attrs">{{ label }}<slot /></label>' },
  UIcon: passthrough,
  UInput: { props: ['modelValue'], template: '<input v-bind="$attrs">' },
  USelectMenu: { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"><slot /></select>' }
}

function formats(overrides: Partial<MediaFormatsResponse> = {}): MediaFormatsResponse {
  return {
    inventory: {
      truncated: false,
      formats: [
        { format_id: '699', kind: 'video', container: 'mp4', video_codec: 'av1', height: 1080, fps: 60, dynamic_range: 'hdr10' },
        { format_id: '137', kind: 'video', container: 'mp4', video_codec: 'avc', height: 1080, fps: 30, dynamic_range: 'sdr' },
        { format_id: '18', kind: 'muxed', container: 'mp4', video_codec: 'avc', audio_codec: 'aac', height: 360, fps: 30, dynamic_range: 'sdr' },
        { format_id: '251', kind: 'audio', container: 'webm', audio_codec: 'opus' }
      ]
    },
    criteria: {
      target: 'video',
      containers: [],
      video_codecs: [],
      audio_codecs: [],
      dynamic_range: [],
      audio_languages: [],
      output: { mode: 'remux', container: 'mp4' },
      allow_merge: true,
      strictness: 'preferred',
      preset: 'best',
      embed: {
        thumbnail: false,
        chapters: false,
        metadata: false,
        info_json: false,
        sponsorblock: { mode: 'off', categories: [] }
      },
      tracks: {
        audio: { extra_languages: [] },
        subtitles: { mode: 'off', languages: [], include_automatic: false, convert_to: null }
      }
    },
    resolved: null,
    unresolved_code: null,
    capabilities: { can_merge: true, can_transcode_audio: true },
    audio_tracks: [{ format_id: '251', language: 'en', codec: 'opus', bitrate_kbps: 128, note: null }],
    subtitles: [{ language: 'en', name: null, source: 'manual', formats: ['vtt'] }],
    ...overrides
  } as MediaFormatsResponse
}

/** The template preview is a server round-trip; the component only renders what it returns. */
const resolveOutput = async () => ({ relative_path: 'clip.mp4', fields: ['title', 'ext'] })

function mount(props: MediaFormatsResponse) {
  return render(MediaFormatSelector, {
    props: { formats: props, resolveOutput },
    global: { plugins: [i18n], components }
  })
}

describe('MediaFormatSelector', () => {
  it('explains an empty result with the per-criterion counts instead of just reporting it', () => {
    // AV1 and HDR both exist on this page, but never in the same format — the useful thing
    // to say is which combination is impossible, not "0 formats".
    mount(
      formats({
        resolved: {
          format_expression: '',
          container: 'mp4',
          estimated_bytes: null,
          label: '',
          relaxations: [],
          warnings: [],
          matched_counts: [
            { criterion: 'video_codec', matched: 6 },
            { criterion: 'dynamic_range', matched: 2 }
          ],
          matched_total: 0,
          candidate_total: 3,
          track_warnings: [],
          embed_warnings: [],
          variant: {} as never
        }
      } as Partial<MediaFormatsResponse>)
    )
    expect(screen.getByTestId('media-no-match')).toBeTruthy()
    expect(screen.getByText(/Each filter matches something on its own/)).toBeTruthy()
    expect(screen.getByText('Video codec alone: 6')).toBeTruthy()
    expect(screen.getByText('Dynamic range alone: 2')).toBeTruthy()
  })

  it('names the criterion to blame when one keeps nothing on its own', () => {
    mount(
      formats({
        resolved: {
          format_expression: '',
          container: 'mp4',
          estimated_bytes: null,
          label: '',
          relaxations: [],
          warnings: [],
          matched_counts: [{ criterion: 'language', matched: 0 }],
          matched_total: 0,
          candidate_total: 3,
          track_warnings: [],
          embed_warnings: [],
          variant: {} as never
        }
      } as Partial<MediaFormatsResponse>)
    )
    expect(screen.getByText(/Nothing on this page satisfies: Language/)).toBeTruthy()
  })

  it('names the page as the reason when it has no audio track, not a filter combination', () => {
    // RD-120-50: the resolver's refusal used to be dropped, and the selector then blamed
    // filters nobody had set.
    mount(formats({ resolved: null, unresolved_code: 'media.audio_missing' }))
    expect(screen.getByText('This page offers nothing to choose from')).toBeTruthy()
    expect(screen.getByTestId('media-unresolved-reason').textContent).toContain('not a single audio track')
    expect(screen.queryByText(/Each filter matches something on its own/)).toBeNull()
  })

  it('says yt-dlp reported no formats when the inventory resolves nothing at all', () => {
    mount(formats({ resolved: null, unresolved_code: 'media.formats_missing' }))
    expect(screen.getByTestId('media-unresolved-reason').textContent).toContain('no downloadable format')
  })

  it('keeps the filter explanation for a refusal that is about the criteria', () => {
    mount(formats({ resolved: null, unresolved_code: 'media.criteria_unsatisfiable' }))
    expect(screen.queryByTestId('media-unresolved-reason')).toBeNull()
    expect(screen.getByText('No format matches')).toBeTruthy()
  })

  it('warns that only progressive formats are usable when ffmpeg is missing', () => {
    mount(formats({ capabilities: { can_merge: false, can_transcode_audio: false } }))
    const warning = screen.getByTestId('media-merge-warning')
    expect(warning.textContent).toContain('ffmpeg')
    // The audio codec filter only makes sense for a merge, so it is disabled rather than
    // silently ignored.
    const audioFilter = screen.getByText('Audio codec').querySelector('select')
    expect(audioFilter?.hasAttribute('disabled')).toBe(true)
  })

  it('keeps the audio codec filter available when ffmpeg is present', () => {
    mount(formats())
    expect(screen.queryByTestId('media-merge-warning')).toBeNull()
    const audioFilter = screen.getByText('Audio codec').querySelector('select')
    expect(audioFilter?.hasAttribute('disabled')).toBe(false)
  })

  it('says which criteria were relaxed rather than silently downgrading', () => {
    mount(
      formats({
        resolved: {
          format_expression: 'bv*+ba/b',
          container: 'mp4',
          estimated_bytes: 1024,
          label: 'AV1 1080p60 · mp4',
          relaxations: ['dynamic_range', 'video_codec'],
          warnings: [],
          matched_counts: [],
          matched_total: 2,
          candidate_total: 3,
          track_warnings: [],
          embed_warnings: [],
          variant: {} as never
        }
      } as Partial<MediaFormatsResponse>)
    )
    const notice = screen.getByTestId('media-relaxations')
    expect(notice.textContent).toContain('Dynamic range')
    expect(notice.textContent).toContain('Video codec')
    expect(screen.getByTestId('media-match-count').textContent).toContain('2 of 3')
  })
})
