<script setup lang="ts">
/**
 * Seeding policy of one torrent, with where each value comes from.
 *
 * Three levels inherit independently — global settings, the package's category, then the
 * torrent itself — so the panel shows the effective value next to its source rather than a
 * single number the user cannot trace.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SeedingPolicyRequest, SeedingPolicyResponse } from '@/api/types'

const { t } = useI18n()
const props = defineProps<{ policy: SeedingPolicyResponse, busy?: boolean }>()
const emit = defineEmits<{ save: [policy: SeedingPolicyRequest], clear: [] }>()

const enabled = ref<boolean | null>(null)
/**
 * Reka UI selects only round-trip string values, so the tri-state override (inherit / yes / no)
 * travels as a string and is mapped back to `boolean | null` here.
 */
const enabledChoice = computed({
  get: () => enabled.value === null ? 'inherit' : enabled.value ? 'yes' : 'no',
  set: (value: string) => { enabled.value = value === 'inherit' ? null : value === 'yes' }
})
const ratio = ref<number | null>(null)
const minutes = ref<number | null>(null)
const unlimited = ref(false)

/** Seeds the form from the stored override, not from the effective values. */
watch(
  () => props.policy,
  (policy) => {
    const stored = policy.torrent_override
    enabled.value = stored?.enabled ?? null
    ratio.value = stored?.ratio_milli != null ? stored.ratio_milli / 1000 : null
    unlimited.value = stored?.time === 'unlimited'
    minutes.value =
      stored?.time && typeof stored.time === 'object' ? stored.time.minutes : null
  },
  { immediate: true }
)

const hasOverride = computed(() => props.policy.torrent_override != null)
const effective = computed(() => props.policy.effective)

/** Human label of the level a value came from. */
function sourceLabel(source: string): string {
  return t(`torrent.seeding.source.${source}`)
}

/** Effective seed time as text, since it is either a number or "unlimited". */
const effectiveTime = computed(() => {
  const time = effective.value.time
  if (time === 'unlimited') return t('torrent.seeding.unlimited')
  return typeof time === 'object' ? t('torrent.seeding.minutes', { count: time.minutes }) : '–'
})

function save(): void {
  emit('save', {
    enabled: enabled.value,
    ratio: ratio.value,
    time_minutes: unlimited.value ? null : minutes.value,
    time_unlimited: unlimited.value ? true : null
  })
}
</script>

<template>
  <div class="grid gap-3">
    <dl class="grid gap-1 text-xs">
      <div class="flex items-baseline gap-2">
        <dt class="w-28 shrink-0 text-toned">{{ t('torrent.seeding.enabled') }}</dt>
        <dd class="flex-1">{{ effective.enabled ? t('common.values.yes') : t('common.values.no') }}</dd>
        <dd class="shrink-0 text-muted">{{ sourceLabel(effective.enabled_source) }}</dd>
      </div>
      <div class="flex items-baseline gap-2">
        <dt class="w-28 shrink-0 text-toned">{{ t('torrent.seeding.ratio') }}</dt>
        <dd class="numeric flex-1">{{ effective.ratio.toFixed(2) }}</dd>
        <dd class="shrink-0 text-muted">{{ sourceLabel(effective.ratio_source) }}</dd>
      </div>
      <div class="flex items-baseline gap-2">
        <dt class="w-28 shrink-0 text-toned">{{ t('torrent.seeding.time') }}</dt>
        <dd class="flex-1">{{ effectiveTime }}</dd>
        <dd class="shrink-0 text-muted">{{ sourceLabel(effective.time_source) }}</dd>
      </div>
    </dl>

    <div class="grid gap-2 border-t border-muted pt-2 sm:grid-cols-2">
      <UFormField :label="t('torrent.seeding.override_enabled')" size="xs">
        <USelect
          v-model="enabledChoice"
          :items="[
            { label: t('torrent.seeding.inherit'), value: 'inherit' },
            { label: t('common.values.yes'), value: 'yes' },
            { label: t('common.values.no'), value: 'no' }
          ]"
          value-key="value"
          :disabled="props.busy"
        />
      </UFormField>
      <UFormField :label="t('torrent.seeding.override_ratio')" size="xs">
        <UInput v-model.number="ratio" type="number" min="0" max="100" step="0.1" :disabled="props.busy" />
      </UFormField>
      <UFormField :label="t('torrent.seeding.override_time')" size="xs">
        <UInput v-model.number="minutes" type="number" min="1" :disabled="props.busy || unlimited">
          <template #trailing><span class="text-xs text-muted">min</span></template>
        </UInput>
      </UFormField>
      <UFormField :label="t('torrent.seeding.override_unlimited')" size="xs">
        <USwitch v-model="unlimited" :disabled="props.busy" />
      </UFormField>
    </div>

    <div class="flex items-center gap-2">
      <UButton size="xs" color="primary" variant="soft" :loading="props.busy" :label="t('torrent.seeding.save')" @click="save" />
      <UButton
        size="xs"
        color="neutral"
        variant="ghost"
        :disabled="props.busy || !hasOverride"
        :label="t('torrent.seeding.clear')"
        @click="emit('clear')"
      />
    </div>
  </div>
</template>
