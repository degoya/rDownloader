<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthCapability, BandwidthProfile, BandwidthStatus, ManualProfileRequest } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { formatBytes, formatLongMoment, formatPauseEnd } from '@/utils/format'

const props = withDefaults(defineProps<{
  /** The profiles the switch offers; the tab has them loaded already. */
  profiles?: BandwidthProfile[]
}>(), { profiles: () => [] })

const { t } = useI18n()
const status = ref<BandwidthStatus | null>(null)
const capabilities = ref<BandwidthCapability[]>([])
let timer: ReturnType<typeof setInterval> | null = null

/** "No limits" in the profile select: Reka's select takes no empty value, so it has a name. */
const NO_LIMITS = 'none'
/** How long a switch by hand holds (RD-190-20): until the schedule changes, an hour or three, or open. */
type SwitchEnd = 'next_switch' | '60' | '180' | 'never'
const chosenProfile = ref<string>(NO_LIMITS)
const chosenEnd = ref<SwitchEnd>('next_switch')
const switching = ref(false)
const switchError = ref<string | null>(null)
// Offers the first profile rather than "no limits" once there is one to offer.
watch(() => props.profiles, (list) => {
  const first = list[0]
  if (chosenProfile.value === NO_LIMITS && first) chosenProfile.value = first.id
}, { immediate: true })

const profileItems = computed(() => [
  ...props.profiles.map(profile => ({ label: profile.name, value: profile.id })),
  { label: t('bandwidth.manual.no_limits'), value: NO_LIMITS }
])
const endItems = computed(() => [
  { label: t('bandwidth.manual.until_next_switch'), value: 'next_switch' },
  { label: t('bandwidth.manual.for_1h'), value: '60' },
  { label: t('bandwidth.manual.for_3h'), value: '180' },
  { label: t('bandwidth.manual.until_back'), value: 'never' }
])
/** Why the active profile is active: the schedule, or a switch by hand and until when. */
const sourceLabel = computed(() => {
  const manual = status.value?.manual
  if (!manual) return t('bandwidth.manual.source_schedule')
  return manual.until
    ? t('bandwidth.manual.source_manual_until', { time: formatPauseEnd(manual.until) })
    : t('bandwidth.manual.source_manual_open')
})

/** The request a choice stands for; the hour choices are a time from now. */
function switchRequest(profile: string, end: SwitchEnd, now: number = Date.now()): ManualProfileRequest {
  const profile_id = profile === NO_LIMITS ? null : profile
  if (end === 'next_switch' || end === 'never') return { profile_id, ends: end }
  return { profile_id, ends: 'at', until: new Date(now + Number(end) * 60_000).toISOString() }
}

async function switchProfile(): Promise<void> {
  switching.value = true
  switchError.value = null
  try {
    const response = await api.PUT('/api/v1/bandwidth/manual', { body: switchRequest(chosenProfile.value, chosenEnd.value) })
    if (response.data) status.value = response.data
    else switchError.value = responseError(response)
  } finally {
    switching.value = false
  }
}

async function backToSchedule(): Promise<void> {
  switching.value = true
  switchError.value = null
  try {
    const response = await api.DELETE('/api/v1/bandwidth/manual')
    if (response.data) status.value = response.data
    else switchError.value = responseError(response)
  } finally {
    switching.value = false
  }
}

/** Anything a limit cannot fully reach; shown so a setting is never silently ignored. */
const partial = computed(() =>
  capabilities.value.filter(entry => !entry.download_enforced || !entry.scoped_enforced)
)

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/bandwidth/status')
  if (response.data) status.value = response.data
}

function usage(used: string, limit: string | null | undefined): string {
  return limit
    ? t('bandwidth.status.usage', { used: formatBytes(used), limit: formatBytes(limit) })
    : formatBytes(used)
}

onMounted(async () => {
  // Armed before the first await: an unmount while the first answers are still on their way
  // runs `onUnmounted` before a later assignment, and the interval would then poll for good.
  timer = setInterval(() => void load(), 10_000)
  await load()
  const response = await api.GET('/api/v1/bandwidth/capabilities')
  if (response.data) capabilities.value = response.data
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

defineExpose({ reload: load })
</script>

<template>
  <UCard v-if="status" as="section">
    <SectionHeader
      :eyebrow="t('bandwidth.status.eyebrow')"
      :title="status.active_profile?.name ?? t('bandwidth.status.no_profile')"
    />
    <p class="mt-1 flex items-center gap-1.5 text-xs text-muted" data-testid="bandwidth-source">
      <UIcon :name="status.manual ? 'i-lucide-hand' : 'i-lucide-calendar-clock'" class="size-3.5 shrink-0" />
      {{ sourceLabel }}
    </p>
    <div class="mt-4 flex flex-wrap items-end gap-2" data-testid="bandwidth-switch">
      <UFormField :label="t('bandwidth.manual.profile')">
        <USelect v-model="chosenProfile" :items="profileItems" value-key="value" class="w-48" />
      </UFormField>
      <UFormField :label="t('bandwidth.manual.ends')">
        <USelect v-model="chosenEnd" :items="endItems" value-key="value" class="w-64" />
      </UFormField>
      <UButton icon="i-lucide-arrow-right-left" color="neutral" variant="outline" :label="t('bandwidth.manual.switch')" :loading="switching" @click="switchProfile" />
      <UButton v-if="status.manual" icon="i-lucide-calendar-clock" color="neutral" variant="ghost" :label="t('bandwidth.manual.back_to_schedule')" :disabled="switching" @click="backToSchedule" />
    </div>
    <UAlert v-if="switchError" class="mt-3" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="switchError" />
    <dl class="mt-4 grid gap-4 sm:grid-cols-2">
      <div>
        <dt class="text-xs text-muted">{{ t('bandwidth.status.binding') }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted">
          <template v-if="status.binding_limit">
            {{ formatBytes(status.binding_limit.bytes_per_second) }}/s
            <span class="text-muted">· {{ t(`bandwidth.source.${status.binding_limit.source}`) }}</span>
          </template>
          <template v-else>{{ t('bandwidth.status.unlimited') }}</template>
        </dd>
      </div>
      <div>
        <dt class="text-xs text-muted">{{ t('bandwidth.status.upload_binding') }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted" data-testid="upload-binding">
          <template v-if="status.upload_binding_limit">
            {{ formatBytes(status.upload_binding_limit.bytes_per_second) }}/s
            <span class="text-muted">· {{ t(`bandwidth.source.${status.upload_binding_limit.source}`) }}</span>
          </template>
          <template v-else>{{ t('bandwidth.status.unlimited') }}</template>
        </dd>
      </div>
      <div>
        <dt class="text-xs text-muted">{{ t('bandwidth.status.next_switch') }}</dt>
        <dd class="mt-1 text-sm text-highlighted">
          {{ formatLongMoment(status.next_switch_at) || t('bandwidth.status.no_switch') }}
          <span class="text-muted">· {{ status.timezone }}</span>
        </dd>
      </div>
      <div v-if="status.daily">
        <dt class="text-xs text-muted">{{ t('bandwidth.status.daily', { period: status.daily.period_key }) }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted">{{ usage(status.daily.used_bytes, status.daily.limit_bytes) }}</dd>
      </div>
      <div v-if="status.monthly">
        <dt class="text-xs text-muted">{{ t('bandwidth.status.monthly', { period: status.monthly.period_key }) }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted">{{ usage(status.monthly.used_bytes, status.monthly.limit_bytes) }}</dd>
      </div>
    </dl>
    <UAlert
      v-if="status.budget_exhausted"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-gauge"
      :description="t('bandwidth.status.budget_exhausted')"
    />
    <USeparator v-if="partial.length" class="my-4" />
    <div v-if="partial.length">
      <p class="text-xs font-medium text-highlighted">{{ t('bandwidth.capabilities.title') }}</p>
      <ul class="mt-2 space-y-1">
        <li v-for="entry in partial" :key="entry.kind" class="flex items-start gap-1.5 text-xs text-muted">
          <UIcon name="i-lucide-info" class="mt-0.5 size-3.5 shrink-0" />
          <span>
            <span class="font-medium text-highlighted">{{ t(`bandwidth.kinds.${entry.kind}`) }}</span>
            — {{ entry.note }}
          </span>
        </li>
      </ul>
    </div>
  </UCard>
</template>
