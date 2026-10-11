<script setup lang="ts">
/**
 * A section of the video and the job's own pauses between requests (RD-1240-15).
 *
 * Both go to yt-dlp as they are (`--download-sections`, `--sleep-requests`,
 * `--sleep-interval`). A position is typed as people write it — `90`, `1:30`, `1:02:03` — and
 * committed only once it reads as a time and the end lies after the start, so a half-typed
 * value never reaches a preview. Pauses left off follow the settings; switched on, they are
 * this link's own, `0` included.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { MediaPauses, MediaSection } from '@/api/types'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { formatTimecode, parseTimecode } from '@/utils/mediaTimecode'
import { WHOLE } from '@/utils/numberInput'

/** Mirrors `MAX_PAUSE_SECONDS` in crates/rd-core/src/media/section.rs. */
const MAX_PAUSE_SECONDS = 600

const { t } = useI18n()
const props = defineProps<{
  section: MediaSection | null
  pauses: MediaPauses | null
  /** Whether ffmpeg is there to cut with; without it the section is offered but disabled. */
  canCut: boolean
  busy?: boolean
}>()
const emit = defineEmits<{
  section: [section: MediaSection | null]
  pauses: [pauses: MediaPauses | null]
}>()

const start = ref(formatTimecode(props.section?.start_seconds))
const end = ref(formatTimecode(props.section?.end_seconds))
watch(() => props.section, value => {
  start.value = formatTimecode(value?.start_seconds)
  end.value = formatTimecode(value?.end_seconds)
})

const startSeconds = computed(() => parseTimecode(start.value))
const endSeconds = computed(() => parseTimecode(end.value))
const problem = computed(() => {
  if (startSeconds.value === undefined || endSeconds.value === undefined) return t('linkgrabber.media.section.invalid')
  if (typeof endSeconds.value === 'number' && (startSeconds.value ?? 0) >= endSeconds.value) return t('linkgrabber.media.section.order')
  return null
})

/** Emits the section once both fields read cleanly; two empty fields are the whole video. */
function commitSection(): void {
  if (problem.value) return
  const startValue = startSeconds.value || null
  const endValue = endSeconds.value ?? null
  const next = startValue === null && endValue === null ? null : { start_seconds: startValue, end_seconds: endValue }
  const current = props.section
  if ((current?.start_seconds ?? null) === (next?.start_seconds ?? null) && (current?.end_seconds ?? null) === (next?.end_seconds ?? null)) return
  emit('section', next)
}

const ownPauses = computed(() => props.pauses !== null)

function toggleOwn(value: boolean | 'indeterminate'): void {
  emit('pauses', value === true ? { sleep_requests_seconds: 0, sleep_interval_seconds: 0 } : null)
}

/** An emptied field is no pause; the server refuses anything past the limit anyway. */
function updatePause(key: keyof MediaPauses, value: number | null | undefined): void {
  if (!props.pauses) return
  const seconds = typeof value === 'number' && Number.isFinite(value) ? Math.min(Math.max(Math.round(value), 0), MAX_PAUSE_SECONDS) : 0
  if (props.pauses[key] === seconds) return
  emit('pauses', { ...props.pauses, [key]: seconds })
}
</script>

<template>
  <div class="flex flex-col gap-3" data-testid="media-section-pauses">
    <UFormField
      :label="t('linkgrabber.media.section.label')"
      :description="t('linkgrabber.media.section.hint')"
    >
      <!-- The word before each time is a badge of its own in a field group, as `NumberWithUnit`
           attaches its unit: in `#leading` it overlapped the placeholder, since the input's
           padding is an icon's width and "From" is wider (RD-1240-28). -->
      <div class="grid gap-2 sm:grid-cols-2">
        <UFieldGroup size="xs" class="flex">
          <UBadge color="neutral" variant="outline" class="shrink-0" data-testid="media-section-start-prefix">{{ t('linkgrabber.media.section.start') }}</UBadge>
          <UInput
            v-model="start"
            :placeholder="t('linkgrabber.media.section.start_placeholder')"
            :disabled="props.busy || !props.canCut"
            :aria-label="t('linkgrabber.media.section.start')"
            class="min-w-0 flex-1"
            data-testid="media-section-start"
            @blur="commitSection"
            @keyup.enter="commitSection"
          />
        </UFieldGroup>
        <UFieldGroup size="xs" class="flex">
          <UBadge color="neutral" variant="outline" class="shrink-0" data-testid="media-section-end-prefix">{{ t('linkgrabber.media.section.end') }}</UBadge>
          <UInput
            v-model="end"
            :placeholder="t('linkgrabber.media.section.end_placeholder')"
            :disabled="props.busy || !props.canCut"
            :aria-label="t('linkgrabber.media.section.end')"
            class="min-w-0 flex-1"
            data-testid="media-section-end"
            @blur="commitSection"
            @keyup.enter="commitSection"
          />
        </UFieldGroup>
      </div>
      <p v-if="problem" class="mt-1 text-xs text-error" data-testid="media-section-error">{{ problem }}</p>
    </UFormField>

    <div class="flex flex-col gap-2">
      <USwitch
        :model-value="ownPauses"
        size="xs"
        :label="t('linkgrabber.media.pauses.own')"
        :description="t('linkgrabber.media.pauses.own_hint')"
        :disabled="props.busy"
        data-testid="media-pauses-own"
        @update:model-value="toggleOwn"
      />
      <div v-if="props.pauses" class="grid gap-2 sm:grid-cols-2">
        <UFormField :label="t('linkgrabber.media.pauses.requests')">
          <NumberWithUnit
            :model-value="props.pauses.sleep_requests_seconds"
            unit="s"
            size="xs"
            :min="0"
            :max="MAX_PAUSE_SECONDS"
            :format-options="WHOLE"
            :disabled="props.busy"
            class="w-full"
            @update:model-value="updatePause('sleep_requests_seconds', $event)"
          />
        </UFormField>
        <UFormField :label="t('linkgrabber.media.pauses.interval')">
          <NumberWithUnit
            :model-value="props.pauses.sleep_interval_seconds"
            unit="s"
            size="xs"
            :min="0"
            :max="MAX_PAUSE_SECONDS"
            :format-options="WHOLE"
            :disabled="props.busy"
            class="w-full"
            @update:model-value="updatePause('sleep_interval_seconds', $event)"
          />
        </UFormField>
      </div>
    </div>
  </div>
</template>
