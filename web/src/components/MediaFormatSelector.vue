<script setup lang="ts">
/**
 * The advanced media format selector.
 *
 * Two rules shape this component. First, it never re-derives what the backend already
 * decided: whether merging is possible comes from `capabilities`, and which formats match
 * comes from a preview request, so the dropdown and the download cannot disagree. Second,
 * an empty result is explained rather than reported — the per-criterion match counts say
 * which combination is impossible, because "0 formats match" is useless to someone who just
 * ticked AV1 and HDR on a page that has both but never together.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  AuthProfile,
  AuthProfileSelection,
  AudioCodecFamily,
  CandidateAuthProfileMode,
  DynamicRange,
  MediaFormatCriteria,
  MediaFormatsResponse,
  MediaEmbedPolicy,
  MediaResolution,
  TrackSelection,
  VideoCodecFamily
} from '@/api/types'
import MediaCookieProfileField from '@/components/MediaCookieProfileField.vue'
import MediaEmbedPolicyCard from '@/components/MediaEmbedPolicyCard.vue'
import MediaOutputTemplateField from '@/components/MediaOutputTemplateField.vue'
import MediaTrackSelector from '@/components/MediaTrackSelector.vue'
import { formatBytes } from '@/utils/format'

const { t } = useI18n()
const props = defineProps<{
  formats: MediaFormatsResponse
  busy?: boolean
  /** Resolves an output template on the server; the parent owns the candidate id. */
  resolveOutput: (template: string) => Promise<{ relative_path: string, fields: string[] } | { error: string }>
  /** Cookie profiles to choose from, and the link they apply to (RD-080-04). */
  authProfiles?: AuthProfile[]
  authSelection?: AuthProfileSelection | null
  url?: string
}>()
const emit = defineEmits<{
  preview: [criteria: MediaFormatCriteria]
  apply: [criteria: MediaFormatCriteria]
  authProfile: [mode: CandidateAuthProfileMode, profileId?: string]
}>()

const PRESETS = ['best', '2160p', '1440p', '1080p', '720p', '480p', 'audio_mp3'] as const
const VIDEO_CODECS: VideoCodecFamily[] = ['avc', 'hevc', 'av1', 'vp9', 'vp8']
const AUDIO_CODECS: AudioCodecFamily[] = ['aac', 'opus', 'vorbis', 'mp3', 'flac', 'ac3', 'eac3']
const RANGES: DynamicRange[] = ['sdr', 'hdr10', 'hdr10_plus', 'hlg', 'dolby_vision']

/**
 * Own copy of the criteria so editing a filter does not mutate the prop.
 *
 * A structured clone would choke on the reactive proxy Vue hands over, and a shallow spread
 * would share the arrays — which is exactly what gets edited here.
 */
function cloneCriteria(value: MediaFormatCriteria): MediaFormatCriteria {
  return {
    ...value,
    containers: [...value.containers],
    video_codecs: [...value.video_codecs],
    audio_codecs: [...value.audio_codecs],
    dynamic_range: [...value.dynamic_range],
    audio_languages: [...value.audio_languages],
    tracks: {
      audio: { ...value.tracks.audio, extra_languages: [...value.tracks.audio.extra_languages] },
      subtitles: { ...value.tracks.subtitles, languages: [...value.tracks.subtitles.languages] }
    },
    embed: {
      ...value.embed,
      sponsorblock: { ...value.embed.sponsorblock, categories: [...value.embed.sponsorblock.categories] }
    }
  }
}

const criteria = ref<MediaFormatCriteria>(cloneCriteria(props.formats.criteria))
const resolution = ref<MediaResolution | null>(props.formats.resolved ?? null)
/** Set when the last preview came back empty; carries nothing the UI can render on its own. */
const unresolved = ref(false)
/**
 * The resolver's stable code when nothing resolves (RD-120-50). "No formats" and "no audio
 * track" are facts about the page, and dressing them up as a filter combination that keeps
 * nothing sent people looking at filters they never set.
 */
const unresolvedCode = ref<string | null>(props.formats.unresolved_code ?? null)

watch(
  () => props.formats,
  value => {
    criteria.value = cloneCriteria(value.criteria)
    resolution.value = value.resolved ?? null
    unresolved.value = false
    unresolvedCode.value = value.unresolved_code ?? null
  }
)

const canMerge = computed(() => props.formats.capabilities.can_merge)
const inventory = computed(() => props.formats.inventory.formats)

/** Containers the page actually offers, so the filter never lists something impossible. */
const containers = computed(() => [...new Set(inventory.value.map(format => format.container).filter(Boolean))].sort())
/** Languages the page actually offers. */
const languages = computed(() => [...new Set(inventory.value.map(format => format.language).filter((value): value is string => Boolean(value)))].sort())

const presetItems = computed(() =>
  PRESETS.map(preset => ({ label: t(`linkgrabber.media.presets.${preset}`), value: preset }))
)
const activePreset = computed(() => criteria.value.preset ?? 'custom')

const containerItems = computed(() => containers.value.map(value => ({ label: value.toUpperCase(), value })))
const videoCodecItems = computed(() => VIDEO_CODECS.map(value => ({ label: t(`linkgrabber.media.codecs.${value}`), value })))
const audioCodecItems = computed(() => AUDIO_CODECS.map(value => ({ label: t(`linkgrabber.media.codecs.${value}`), value })))
const rangeItems = computed(() => RANGES.map(value => ({ label: t(`linkgrabber.media.ranges.${value}`), value })))
const languageItems = computed(() => languages.value.map(value => ({ label: value.toUpperCase(), value })))

const matched = computed(() => resolution.value?.matched_total ?? 0)
const total = computed(() => resolution.value?.candidate_total ?? inventory.value.length)
const estimated = computed(() => {
  const bytes = resolution.value?.estimated_bytes
  return bytes ? formatBytes(BigInt(bytes)) : null
})

/** Criteria that kept nothing on their own — the ones actually to blame for an empty result. */
const blocking = computed(() => (resolution.value?.matched_counts ?? []).filter(entry => entry.matched === 0))
/** Every criterion with its isolated count, for the "AV1 alone: 6, HDR alone: 2" explanation. */
const counts = computed(() => resolution.value?.matched_counts ?? [])
/** Reasons that are about the page rather than the filters, each with its own explanation. */
const PAGE_REASONS: Record<string, string> = {
  'media.formats_missing': 'linkgrabber.media.unresolved.formats_missing',
  'media.audio_missing': 'linkgrabber.media.unresolved.audio_missing',
  'media.merge_unavailable': 'linkgrabber.media.merge_unavailable'
}
const pageReason = computed(() => (unresolvedCode.value ? PAGE_REASONS[unresolvedCode.value] ?? null : null))
const relaxations = computed(() => resolution.value?.relaxations ?? [])
const trackWarnings = computed(() => resolution.value?.track_warnings ?? [])
const embedWarnings = computed(() => resolution.value?.embed_warnings ?? [])
const warnings = computed(() => resolution.value?.warnings ?? [])

function selectPreset(preset: string): void {
  criteria.value = { ...criteria.value, preset: preset === 'custom' ? null : preset }
  emit('preview', criteria.value)
}

/** Any manual change makes the selection custom; the preset chip is only a starting point. */
function touched(): void {
  criteria.value = { ...criteria.value, preset: null }
  emit('preview', criteria.value)
}

function updateTracks(tracks: TrackSelection): void {
  criteria.value = { ...criteria.value, tracks, preset: null }
  emit('preview', criteria.value)
}

function updateEmbed(embed: MediaEmbedPolicy): void {
  criteria.value = { ...criteria.value, embed, preset: null }
  emit('preview', criteria.value)
}

function updateTemplate(template: string | null): void {
  criteria.value = { ...criteria.value, output_template: template, preset: null }
  emit('preview', criteria.value)
}

function apply(): void {
  emit('apply', criteria.value)
}

/** Replaces the current resolution; called by the parent once a preview returns. */
function setResolution(value: MediaResolution | null, code: string | null = null): void {
  resolution.value = value
  unresolved.value = value === null
  unresolvedCode.value = value === null ? code : null
}

defineExpose({ setResolution })
</script>

<template>
  <div class="flex flex-col gap-4" data-testid="media-format-selector">
    <div class="flex flex-wrap items-center gap-1.5">
      <UButton
        v-for="item in presetItems"
        :key="item.value"
        :label="item.label"
        size="xs"
        :color="activePreset === item.value ? 'primary' : 'neutral'"
        :variant="activePreset === item.value ? 'soft' : 'ghost'"
        :disabled="props.busy"
        @click="selectPreset(item.value)"
      />
      <UBadge v-if="activePreset === 'custom'" color="primary" variant="subtle" size="sm">
        {{ t('linkgrabber.media.presets.custom') }}
      </UBadge>
    </div>

    <UAlert
      v-if="!canMerge"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :title="t('linkgrabber.media.merge_unavailable_title')"
      :description="t('linkgrabber.media.merge_unavailable')"
      data-testid="media-merge-warning"
    />

    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField :label="t('linkgrabber.media.filters.container')">
        <USelectMenu
          v-model="criteria.containers"
          :items="containerItems"
          value-key="value"
          multiple
          size="xs"
          :disabled="props.busy"
          :placeholder="t('linkgrabber.media.filters.any')"
          @update:model-value="touched"
        />
      </UFormField>
      <UFormField :label="t('linkgrabber.media.filters.video_codec')">
        <USelectMenu
          v-model="criteria.video_codecs"
          :items="videoCodecItems"
          value-key="value"
          multiple
          size="xs"
          :disabled="props.busy"
          :placeholder="t('linkgrabber.media.filters.any')"
          @update:model-value="touched"
        />
      </UFormField>
      <UFormField :label="t('linkgrabber.media.filters.audio_codec')">
        <USelectMenu
          v-model="criteria.audio_codecs"
          :items="audioCodecItems"
          value-key="value"
          multiple
          size="xs"
          :disabled="props.busy || !canMerge"
          :placeholder="t('linkgrabber.media.filters.any')"
          @update:model-value="touched"
        />
      </UFormField>
      <UFormField :label="t('linkgrabber.media.filters.dynamic_range')">
        <USelectMenu
          v-model="criteria.dynamic_range"
          :items="rangeItems"
          value-key="value"
          multiple
          size="xs"
          :disabled="props.busy"
          :placeholder="t('linkgrabber.media.filters.any')"
          @update:model-value="touched"
        />
      </UFormField>
      <UFormField :label="t('linkgrabber.media.filters.max_height')">
        <UInput v-model.number="criteria.max_height" type="number" min="0" size="xs" :disabled="props.busy" @change="touched" />
      </UFormField>
      <UFormField :label="t('linkgrabber.media.filters.max_fps')">
        <UInput v-model.number="criteria.max_fps" type="number" min="0" size="xs" :disabled="props.busy" @change="touched" />
      </UFormField>
      <UFormField :label="t('linkgrabber.media.filters.max_bitrate')">
        <UInput v-model.number="criteria.max_total_bitrate_kbps" type="number" min="0" size="xs" :disabled="props.busy" @change="touched" />
      </UFormField>
      <UFormField v-if="languageItems.length" :label="t('linkgrabber.media.filters.language')">
        <USelectMenu
          v-model="criteria.audio_languages"
          :items="languageItems"
          value-key="value"
          multiple
          size="xs"
          :disabled="props.busy"
          :placeholder="t('linkgrabber.media.filters.any')"
          @update:model-value="touched"
        />
      </UFormField>
    </div>

    <MediaTrackSelector
      :tracks="criteria.tracks"
      :audio-tracks="props.formats.audio_tracks"
      :subtitles="props.formats.subtitles"
      :warnings="trackWarnings"
      :can-merge="canMerge"
      :busy="props.busy"
      @change="updateTracks"
    />

    <MediaEmbedPolicyCard
      :embed="criteria.embed"
      :warnings="embedWarnings"
      :can-transcode="props.formats.capabilities.can_transcode_audio"
      :busy="props.busy"
      @change="updateEmbed"
    />

    <MediaCookieProfileField
      v-if="props.url"
      :selection="props.authSelection ?? null"
      :profiles="props.authProfiles ?? []"
      :url="props.url"
      :busy="props.busy"
      @change="(mode, profileId) => emit('authProfile', mode, profileId)"
    />

    <MediaOutputTemplateField
      :template="criteria.output_template ?? null"
      :resolve="props.resolveOutput"
      :busy="props.busy"
      @change="updateTemplate"
    />

    <div class="flex flex-wrap items-center gap-2 text-sm">
      <UBadge color="neutral" variant="subtle" data-testid="media-match-count">
        {{ t('linkgrabber.media.matched', { matched, total }) }}
      </UBadge>
      <span v-if="resolution" class="text-muted">{{ resolution.label }}</span>
      <span v-if="estimated" class="text-muted">≈{{ estimated }}</span>
    </div>

    <UAlert
      v-if="unresolved || matched === 0"
      color="warning"
      variant="subtle"
      icon="i-lucide-filter-x"
      :title="pageReason ? t('linkgrabber.media.unresolved_title') : t('linkgrabber.media.no_match_title')"
      data-testid="media-no-match"
    >
      <template #description>
        <p v-if="pageReason" data-testid="media-unresolved-reason">{{ t(pageReason) }}</p>
        <p v-else-if="blocking.length">
          {{ t('linkgrabber.media.no_match_single', { criteria: blocking.map(entry => t(`linkgrabber.media.criteria.${entry.criterion}`)).join(', ') }) }}
        </p>
        <p v-else>{{ t('linkgrabber.media.no_match_combination') }}</p>
        <ul v-if="!pageReason && counts.length" class="mt-1 list-inside list-disc">
          <li v-for="entry in counts" :key="entry.criterion">
            {{ t('linkgrabber.media.criterion_count', { criterion: t(`linkgrabber.media.criteria.${entry.criterion}`), matched: entry.matched }) }}
          </li>
        </ul>
      </template>
    </UAlert>

    <UAlert
      v-if="relaxations.length"
      color="info"
      variant="subtle"
      icon="i-lucide-info"
      data-testid="media-relaxations"
      :description="t('linkgrabber.media.relaxed', { criteria: relaxations.map(name => t(`linkgrabber.media.criteria.${name}`)).join(', ') })"
    />

    <UAlert
      v-for="(warning, index) in warnings"
      :key="index"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :description="warning.kind === 'codec_container_mismatch'
        ? t('linkgrabber.media.warnings.codec_container_mismatch', { codec: warning.codec, container: warning.container })
        : t(`linkgrabber.media.warnings.${warning.kind}`)"
    />

    <div class="flex justify-end">
      <UButton
        :label="t('linkgrabber.media.apply')"
        color="primary"
        size="sm"
        :disabled="props.busy || matched === 0"
        :loading="props.busy"
        @click="apply"
      />
    </div>
  </div>
</template>
