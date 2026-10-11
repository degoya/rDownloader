import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { MediaFormatCriteria, MediaFormatsResponse } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import MediaFormatSelector from './MediaFormatSelector.vue'

const USelectMenu = { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"><slot /></select>' }

/** A hand-built selection with every format filter set, so a preset that keeps one shows. */
const custom: MediaFormatCriteria = {
  target: 'video',
  containers: ['webm'],
  video_codecs: ['av1'],
  audio_codecs: ['opus'],
  dynamic_range: ['hdr10'],
  min_height: 360,
  max_height: 720,
  min_fps: 24,
  max_fps: 30,
  max_total_bitrate_kbps: 4000,
  min_audio_bitrate_kbps: 96,
  audio_languages: ['de'],
  output: { mode: 'passthrough' },
  allow_merge: false,
  strictness: 'required',
  output_template: '{title}.{ext}',
  preset: null,
  section: null,
  pauses: null,
  embed: {
    thumbnail: true,
    chapters: false,
    metadata: false,
    info_json: false,
    sponsorblock: { mode: 'off', categories: [] }
  },
  tracks: {
    audio: { extra_languages: ['en'] },
    subtitles: { mode: 'off', languages: [], include_automatic: false, convert_to: null }
  }
}

function formats(): MediaFormatsResponse {
  return {
    inventory: { truncated: false, formats: [] },
    criteria: custom,
    resolved: null,
    unresolved_code: null,
    capabilities: { can_merge: true, can_transcode_audio: true },
    audio_tracks: [],
    subtitles: []
  } as unknown as MediaFormatsResponse
}

/** What every preset resets: the format filters back to "any", as `rd-core` defines them. */
const RESET = {
  target: 'video',
  containers: [],
  video_codecs: [],
  audio_codecs: [],
  dynamic_range: [],
  min_height: null,
  max_height: null,
  min_fps: null,
  max_fps: null,
  max_total_bitrate_kbps: null,
  min_audio_bitrate_kbps: null,
  audio_languages: [],
  output: { mode: 'remux', container: 'mp4' },
  allow_merge: true,
  strictness: 'preferred'
}

/** RD-1240-21: a preset chip sets the criteria its name promises, not just the label. */
describe('MediaFormatSelector presets', () => {
  it.each([
    ['best', {}],
    ['2160p', { max_height: 2160 }],
    ['1440p', { max_height: 1440 }],
    ['1080p', { max_height: 1080 }],
    ['720p', { max_height: 720 }],
    ['480p', { max_height: 480 }],
    ['audio_mp3', { target: 'audio_only', output: { mode: 'extract_audio', codec: 'mp3', quality: 0 } }]
  ])('%s sends its own criteria', async (preset, own) => {
    const { emitted } = mountComponent(MediaFormatSelector, {
      messages: { linkgrabber: en },
      props: { formats: formats(), resolveOutput: async () => ({ relative_path: 'clip.mp4', fields: [] }) },
      stubs: { USelectMenu }
    })
    const group = screen.getByRole('group', { name: en.media.preset_label })
    const label = en.media.presets[preset as keyof typeof en.media.presets]
    await fireEvent.click(within(group).getByRole('radio', { name: label }))

    const [sent] = (emitted().preview as MediaFormatCriteria[][]).at(-1) ?? []
    expect(sent).toEqual({
      ...RESET,
      ...own,
      preset,
      // Not part of a preset: they stay as the person set them.
      output_template: custom.output_template,
      embed: custom.embed,
      tracks: custom.tracks,
      section: custom.section,
      pauses: custom.pauses
    })
  })
})
