<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PostprocessStep } from '@/api/types'
import { translateServerMessage } from '@/i18n/server'

const props = defineProps<{ steps: PostprocessStep[], loading?: boolean }>()

const { t, te } = useI18n()
// Mirrors `PostprocessKind`. A kind missing here falls back to the raw identifier, which is
// how `remux` went untranslated since livestream remuxing arrived.
const KINDS = ['par2', 'sfv', 'rar_test', 'extract_zip', 'extract_seven_zip', 'extract_rar', 'delete_archives', 'delete_par2', 'cleanup', 'remux', 'plugin_step', 'script', 'upload']
const STATES = ['queued', 'running', 'completed', 'skipped', 'failed']
const openOutput = ref<Set<string>>(new Set())

const ordered = computed(() => [...props.steps].sort((left, right) => (left.position ?? 0) - (right.position ?? 0)))

function kindLabel(kind: string): string {
  return KINDS.includes(kind) && te(`downloads.postprocess.kinds.${kind}`) ? t(`downloads.postprocess.kinds.${kind}`) : kind
}
function stateLabel(state: string): string {
  return STATES.includes(state) && te(`downloads.postprocess.states.${state}`) ? t(`downloads.postprocess.states.${state}`) : state
}
const stateColor: Record<string, 'neutral' | 'primary' | 'success' | 'warning' | 'error'> = {
  queued: 'neutral', running: 'primary', completed: 'success', skipped: 'warning', failed: 'error'
}

/**
 * What a step says about its outcome, in the reader's language where it can be.
 *
 * One way, for every step: the stable code (RD-107-04) is translated through
 * `server.codes.<code>` with its own parameters, and the English text the server wrote is the
 * fallback for a code this build does not know. Extraction used to carry its code inside that
 * text and be taken apart again here; it carries the code in the field now (RD-108-08).
 */
function stepMessage(step: PostprocessStep): string {
  // `exactOptionalPropertyTypes` is on: a property that is present may not be `undefined`, and
  // the generated schema makes all three optional. `?? null` is the absence this type spells.
  return translateServerMessage({
    message: step.message ?? null,
    code: step.code ?? null,
    params: step.params ?? null
  })
}

function stepKey(step: PostprocessStep): string {
  return `${step.kind}:${step.source_path}`
}
function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}
function hasProgress(step: PostprocessStep): boolean {
  return step.state === 'running' && step.progress_percent !== null && step.progress_percent !== undefined
}
function toggleOutput(key: string): void {
  const next = new Set(openOutput.value)
  next.has(key) ? next.delete(key) : next.add(key)
  openOutput.value = next
}
</script>

<template>
  <div class="space-y-1 text-xs">
    <p v-if="props.loading" class="text-muted">{{ t('downloads.postprocess.loading') }}</p>
    <p v-else-if="!props.steps.length" class="text-muted">{{ t('downloads.postprocess.empty') }}</p>
    <div v-for="step in ordered" :key="stepKey(step)" class="grid gap-1 bg-elevated p-2 sm:grid-cols-[auto_1fr_auto] sm:items-center">
      <UBadge :color="stateColor[step.state] ?? 'neutral'" variant="subtle" size="sm">{{ stateLabel(step.state) }}</UBadge>
      <div class="min-w-0">
        <p class="truncate text-highlighted" :title="step.source_path">{{ kindLabel(step.kind) }} · {{ fileName(step.source_path) }}</p>
        <div v-if="hasProgress(step)" class="mt-1 flex items-center gap-2">
          <UProgress
            :model-value="step.progress_percent ?? 0"
            size="xs"
            class="flex-1"
            :aria-label="t('downloads.postprocess.progress_label', { step: kindLabel(step.kind) })"
          />
          <span class="numeric w-9 text-right text-[11px] text-toned">{{ step.progress_percent }}%</span>
        </div>
        <p v-if="step.output_path" class="truncate font-mono text-[11px] text-muted" :title="step.output_path">→ {{ step.output_path }}</p>
        <template v-if="step.message && step.kind === 'script'">
          <UButton size="xs" color="neutral" variant="link" class="px-0" :icon="openOutput.has(stepKey(step)) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" :label="openOutput.has(stepKey(step)) ? t('downloads.postprocess.hide_output') : t('downloads.postprocess.show_output')" @click="toggleOutput(stepKey(step))" />
          <pre v-if="openOutput.has(stepKey(step))" class="max-h-64 overflow-auto whitespace-pre-wrap text-xs" :class="step.state === 'failed' ? 'text-error' : 'text-muted'">{{ step.message }}</pre>
        </template>
        <p v-else-if="step.message || step.code" class="truncate" :class="step.state === 'failed' ? 'text-error' : 'text-muted'" :title="stepMessage(step)">{{ stepMessage(step) }}</p>
      </div>
      <span class="numeric text-[11px] text-muted">{{ new Date(step.updated_at).toLocaleTimeString() }}</span>
    </div>
  </div>
</template>
