<script setup lang="ts">
/**
 * Audio tracks and subtitles for one media link (RD-080-02).
 *
 * Manual and automatic subtitles are listed apart and counted apart. An automatic track is a
 * speech-recognition guess — routinely wrong about names, numbers and negations — and once
 * it is embedded in a file it is indistinguishable from an authored translation. So it is
 * never implied: using one takes a deliberate toggle, and the label says what it is.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AudioTrack, SubtitleMode, SubtitleTrack, TrackSelection, TrackWarning } from '@/api/types'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'

const { t } = useI18n()
const props = defineProps<{
  tracks: TrackSelection
  audioTracks: AudioTrack[]
  subtitles: SubtitleTrack[]
  warnings?: TrackWarning[]
  canMerge: boolean
  busy?: boolean
}>()
const emit = defineEmits<{ change: [tracks: TrackSelection] }>()

const MODES: SubtitleMode[] = ['off', 'sidecar', 'embed', 'sidecar_and_embed']
const FORMATS = ['srt', 'vtt', 'ass'] as const

const modeItems = computed(() => MODES.map(value => ({ label: t(`linkgrabber.media.tracks.modes.${value}`), value })))
const formatItems = computed(() => FORMATS.map(value => ({ label: value.toUpperCase(), value })))

/** Audio languages the page actually offers, so the filter never lists an impossible one. */
const audioLanguages = computed(() =>
  [...new Set(props.audioTracks.map(track => track.language).filter((value): value is string => Boolean(value)))]
    .sort()
    .map(value => ({ label: value.toUpperCase(), value }))
)

/** Subtitle languages, labelled with whether a manual track exists for them. */
const subtitleLanguages = computed(() => {
  const manual = new Set(props.subtitles.filter(track => track.source === 'manual').map(track => track.language))
  return [...new Set(props.subtitles.map(track => track.language))].sort().map(value => ({
    label: manual.has(value) ? value.toUpperCase() : t('linkgrabber.media.tracks.automatic_only', { language: value.toUpperCase() }),
    value
  }))
})

const hasAutomaticOnly = computed(() =>
  props.subtitles.some(track => track.source === 'automatic')
  && props.subtitles.every(track => track.source === 'automatic')
)
const subtitlesOff = computed(() => props.tracks.subtitles.mode === 'off')
/** Embedding needs a remux; a sidecar file does not. */
const embedDisabled = computed(() => !props.canMerge)

function update(patch: Partial<TrackSelection>): void {
  emit('change', { ...props.tracks, ...patch })
}

function setMode(mode: SubtitleMode): void {
  update({ subtitles: { ...props.tracks.subtitles, mode } })
}

function setSubtitleLanguages(languages: string[]): void {
  update({ subtitles: { ...props.tracks.subtitles, languages } })
}

function setAutomatic(include: boolean): void {
  update({ subtitles: { ...props.tracks.subtitles, include_automatic: include } })
}

function setConvert(value: string): void {
  update({ subtitles: { ...props.tracks.subtitles, convert_to: selectionValue(value) } })
}

function setExtraAudio(languages: string[]): void {
  update({ audio: { ...props.tracks.audio, extra_languages: languages } })
}

/** A warning's translated text; the codec/container ones carry parameters. */
function warningText(warning: TrackWarning): string {
  switch (warning.kind) {
    case 'multiple_audio_unsupported':
    case 'subtitle_embed_unsupported':
      return t(`linkgrabber.media.tracks.warnings.${warning.kind}`, { container: warning.container.toUpperCase() })
    case 'language_unavailable':
    case 'only_automatic_available':
      return t(`linkgrabber.media.tracks.warnings.${warning.kind}`, { language: warning.language.toUpperCase() })
    default:
      return t('linkgrabber.media.tracks.warnings.tool_unavailable')
  }
}
</script>

<template>
  <div class="flex flex-col gap-3" data-testid="media-track-selector">
    <UFormField v-if="audioLanguages.length > 1" :label="t('linkgrabber.media.tracks.extra_audio')">
      <USelectMenu
        :model-value="props.tracks.audio.extra_languages"
        :items="audioLanguages"
        value-key="value"
        multiple
        size="xs"
        :disabled="props.busy || !props.canMerge"
        :placeholder="t('linkgrabber.media.tracks.none')"
        data-testid="media-extra-audio"
        @update:model-value="setExtraAudio"
      />
    </UFormField>

    <UFormField :label="t('linkgrabber.media.tracks.subtitles')">
      <div class="flex flex-wrap items-center gap-1.5">
        <UButton
          v-for="item in modeItems"
          :key="item.value"
          :label="item.label"
          size="xs"
          :color="props.tracks.subtitles.mode === item.value ? 'primary' : 'neutral'"
          :variant="props.tracks.subtitles.mode === item.value ? 'soft' : 'ghost'"
          :disabled="props.busy || (embedDisabled && (item.value === 'embed' || item.value === 'sidecar_and_embed'))"
          :title="embedDisabled && (item.value === 'embed' || item.value === 'sidecar_and_embed')
            ? t('linkgrabber.media.tracks.embed_needs_ffmpeg')
            : undefined"
          @click="setMode(item.value)"
        />
      </div>
    </UFormField>

    <template v-if="!subtitlesOff">
      <UFormField :label="t('linkgrabber.media.tracks.subtitle_languages')">
        <USelectMenu
          :model-value="props.tracks.subtitles.languages"
          :items="subtitleLanguages"
          value-key="value"
          multiple
          size="xs"
          :disabled="props.busy"
          :placeholder="t('linkgrabber.media.tracks.all_languages')"
          data-testid="media-subtitle-languages"
          @update:model-value="setSubtitleLanguages"
        />
      </UFormField>
      <UCheckbox
        :model-value="props.tracks.subtitles.include_automatic"
        :label="t('linkgrabber.media.tracks.include_automatic')"
        :description="t('linkgrabber.media.tracks.include_automatic_hint')"
        :disabled="props.busy"
        data-testid="media-include-automatic"
        @update:model-value="(value: boolean | 'indeterminate') => setAutomatic(value === true)"
      />
      <UFormField :label="t('linkgrabber.media.tracks.convert_to')">
        <USelect
          :model-value="optionalSelection(props.tracks.subtitles.convert_to)"
          :items="[{ label: t('linkgrabber.media.tracks.keep_format'), value: NO_SELECTION }, ...formatItems]"
          value-key="value"
          size="xs"
          :disabled="props.busy"
          @update:model-value="(value: string) => setConvert(value)"
        />
      </UFormField>
    </template>

    <UAlert
      v-if="hasAutomaticOnly && !subtitlesOff && !props.tracks.subtitles.include_automatic"
      color="warning"
      variant="subtle"
      icon="i-lucide-captions-off"
      data-testid="media-automatic-only"
      :description="t('linkgrabber.media.tracks.automatic_only_hint')"
    />
    <UAlert
      v-for="(warning, index) in props.warnings ?? []"
      :key="index"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      data-testid="media-track-warning"
      :description="warningText(warning)"
    />
  </div>
</template>
