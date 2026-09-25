/**
 * Fixed stream names offered by the recording UI.
 *
 * Streamlink plugins expose stream names such as `1080p60`, while `best`, `worst` and
 * `audio_only` are common aliases. Keeping the list here prevents the channel form and the
 * global default from drifting apart or passing arbitrary text to the executable.
 */
const STREAM_QUALITY_PRESETS = [
  'best',
  '2160p60',
  '2160p',
  '1440p60',
  '1440p',
  '1080p60',
  '1080p',
  '720p60',
  '720p',
  '480p',
  '360p',
  '240p',
  '160p',
  'audio_only',
  'worst'
] as const

export function streamQualityItems(): { label: string, value: string }[] {
  return STREAM_QUALITY_PRESETS.map(value => ({ label: value, value }))
}
