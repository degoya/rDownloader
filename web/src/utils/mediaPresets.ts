import type { MediaFormatCriteria } from '@/api/types'

/**
 * The criteria a format preset stands for, applied to the current selection (RD-1240-21).
 *
 * The server reads `preset` as a label only — the criteria are the contract — so a preset chip
 * that changed nothing but the label sent the old filters under a new name. This mirrors
 * `MediaFormatCriteria::preset` in `rd-core`: every format filter goes back to "any", then the
 * preset sets its own. Tracks, embedding and the output template are not part of a preset and
 * stay as they are.
 */
export function presetCriteria(preset: string, current: MediaFormatCriteria): MediaFormatCriteria {
  const base: MediaFormatCriteria = {
    ...current,
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
    strictness: 'preferred',
    preset
  }
  if (preset === 'audio_mp3') {
    return { ...base, target: 'audio_only', output: { mode: 'extract_audio', codec: 'mp3', quality: 0 } }
  }
  const height = /^(\d+)p$/.exec(preset)
  return height ? { ...base, max_height: Number(height[1]) } : base
}
