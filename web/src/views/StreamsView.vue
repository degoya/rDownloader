<script setup lang="ts">
import { storeToRefs } from 'pinia'
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Category, StreamChannel, StreamChannelRequest, StreamSchedule, StreamScheduleRequest } from '@/api/types'
import TimezoneSelect from '@/components/TimezoneSelect.vue'
import { useEditableList } from '@/composables/useEditableList'
import { useStreamsStore } from '@/stores/streams'
import { streamQualityItems } from '@/utils/streamQuality'
import { browserTimezone } from '@/utils/timezones'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'
import { formatMoment } from '@/utils/format'

const { t } = useI18n()
// Shared with the nav badge, so every add/remove here keeps the sidebar count in sync.
const streams = useStreamsStore()
const { channels, schedules, runs } = storeToRefs(streams)
const categories = ref<Category[]>([])
const loading = ref(true)
const message = ref<string | null>(null)

const NONE = '__none__'
const DEFAULT_QUALITY = '__default__'
interface ChannelForm {
  url: string
  name: string | null
  quality: string | null
  category_id: string | null
  enabled: boolean
  // RD-080-09. Splitting is entered as a mode plus a number rather than a tagged union,
  // because a form is a flat thing and the union is rebuilt on submit.
  splitMode: 'none' | 'duration' | 'size'
  splitValue: number
  remux: 'none' | 'mkv' | 'mp4'
  sidecarMetadata: boolean
  sidecarThumbnail: boolean
  reconnectDelay: number
}

function emptyChannel(): ChannelForm {
  return {
    url: '',
    name: null,
    quality: null,
    category_id: null,
    enabled: true,
    splitMode: 'none',
    splitValue: 60,
    remux: 'none',
    sidecarMetadata: false,
    sidecarThumbnail: false,
    reconnectDelay: 5
  }
}

const form = reactive<ChannelForm>(emptyChannel())

const splitItems = computed(() => [
  { value: 'none', label: t('streams.recording.split_none') },
  { value: 'duration', label: t('streams.recording.split_duration') },
  { value: 'size', label: t('streams.recording.split_size') }
])

const remuxItems = computed(() => [
  { value: 'none', label: t('streams.recording.remux_none') },
  { value: 'mkv', label: 'MKV' },
  { value: 'mp4', label: 'MP4' }
])

/** Rebuilds the tagged union the API expects from the flat form fields. */
const list = useEditableList<StreamChannel, StreamChannelRequest>({
  list: channels,
  create: body => api.POST('/api/v1/streams/channels', { body }),
  update: (id, body) => api.PUT('/api/v1/streams/channels/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/streams/channels/{id}', { params: { path: { id } } }),
  reset: () => Object.assign(form, emptyChannel()),
  confirmDelete: channel => ({
    title: t('streams.delete.title'),
    description: t('streams.delete.description', { name: channel.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

function recordingPolicy() {
  const split =
    form.splitMode === 'duration'
      ? { mode: 'duration', value: form.splitValue }
      : form.splitMode === 'size'
        ? { mode: 'size', value: form.splitValue }
        : { mode: 'none' }
  return {
    split,
    remux: form.remux,
    sidecars: {
      metadata: form.sidecarMetadata,
      thumbnail: form.sidecarThumbnail,
      subtitles: false,
      chat: false
    },
    vod_fallback: { mode: 'off' },
    reconnect_delay_seconds: form.reconnectDelay
  }
}
const category = computed({
  get: () => form.category_id ?? NONE,
  set: (value: string) => { form.category_id = value === NONE ? null : value }
})
const categoryItems = computed(() => [
  { label: t('streams.form.category_default'), value: NONE },
  ...categories.value.map(item => ({ label: item.name, value: item.id }))
])
const quality = computed({
  get: () => form.quality ?? DEFAULT_QUALITY,
  set: (value: string) => { form.quality = value === DEFAULT_QUALITY ? null : value }
})
const qualityItems = computed(() => [
  { label: t('streams.quality_default'), value: DEFAULT_QUALITY },
  ...streamQualityItems()
])

onMounted(async () => {
  const [, , categoriesResponse] = await Promise.all([
    streams.refresh(),
    streams.refreshSchedules(),
    api.GET('/api/v1/categories')
  ])
  loading.value = false
  if (streams.error) error.value = streams.error
  if (categoriesResponse.data) categories.value = categoriesResponse.data
})

function body(): StreamChannelRequest {
  return {
    url: form.url.trim(),
    name: form.name?.trim() || null,
    quality: form.quality?.trim() || null,
    category_id: form.category_id,
    enabled: form.enabled,
    recording: recordingPolicy()
  } as unknown as StreamChannelRequest
}

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit(body())
  if (saved) message.value = updating ? t('streams.messages.updated') : t('streams.messages.created')
}

async function recordNow(url: string, name?: string | null, quality?: string | null, categoryId?: string | null): Promise<void> {
  error.value = null
  message.value = null
  const response = await api.POST('/api/v1/streams/record', {
    body: { url, name: name ?? null, quality: quality ?? null, category_id: categoryId ?? null }
  })
  if (!response.data) return void (error.value = responseError(response))
  message.value = t('streams.messages.recording_started', { name: response.data.name })
}

function edit(channel: StreamChannel): void {
  list.edit(channel)
  form.url = channel.url
  form.name = channel.name
  form.quality = channel.quality ?? null
  form.category_id = channel.category_id ?? null
  form.enabled = channel.enabled
  const recording = channel.recording
  const split = recording?.split as { mode?: string, value?: number } | undefined
  form.splitMode = (split?.mode as ChannelForm['splitMode']) ?? 'none'
  form.splitValue = split?.value ?? 60
  form.remux = (recording?.remux as ChannelForm['remux']) ?? 'none'
  form.sidecarMetadata = recording?.sidecars?.metadata ?? false
  form.sidecarThumbnail = recording?.sidecars?.thumbnail ?? false
  form.reconnectDelay = recording?.reconnect_delay_seconds ?? 5
}

async function toggle(channel: StreamChannel, enabled: boolean): Promise<void> {
  const response = await api.PUT('/api/v1/streams/channels/{id}', {
    params: { path: { id: channel.id } },
    body: { url: channel.url, name: channel.name, quality: channel.quality ?? null, category_id: channel.category_id ?? null, enabled }
  })
  if (!response.data) return void (error.value = responseError(response))
  channels.value = channels.value.map(item => item.id === response.data.id ? response.data : item)
}

async function remove(channel: StreamChannel): Promise<void> {
  await list.remove(channel)
}

// ---- Schedules (RD-080-08) ----

const WEEKDAYS = [1, 2, 3, 4, 5, 6, 7] as const

interface ScheduleForm {
  id: string | null
  channelId: string
  name: string
  days: number[]
  startTime: string
  timezone: string
  windowMinutes: number
  leadMinutes: number
  trailMinutes: number
  replayFromStart: boolean
}

function emptySchedule(): ScheduleForm {
  return {
    id: null,
    channelId: '',
    name: '',
    days: [],
    startTime: '20:00',
    // The browser's own zone is almost always the one meant, and an IANA name is what the
    // server needs — an offset would be wrong for half the year.
    timezone: browserTimezone(),
    windowMinutes: 120,
    leadMinutes: 5,
    trailMinutes: 10,
    replayFromStart: false
  }
}

const schedule = reactive<ScheduleForm>(emptySchedule())

const channelItems = computed(() =>
  channels.value.map(entry => ({ value: entry.id, label: entry.name }))
)

function toggleDay(day: number): void {
  schedule.days = schedule.days.includes(day)
    ? schedule.days.filter(entry => entry !== day)
    : [...schedule.days, day].sort((a, b) => a - b)
}

/** `HH:MM` to minutes after local midnight, which is how the server stores it. */
function startMinute(value: string): number {
  const [hours = '0', minutes = '0'] = value.split(':')
  return Number(hours) * 60 + Number(minutes)
}

function minuteToTime(value: number): string {
  const hours = String(Math.floor(value / 60)).padStart(2, '0')
  const minutes = String(value % 60).padStart(2, '0')
  return `${hours}:${minutes}`
}

async function submitSchedule(): Promise<void> {
  const body = {
    channel_id: schedule.channelId,
    name: schedule.name.trim(),
    enabled: true,
    kind: 'weekly',
    days: schedule.days,
    start_minute: startMinute(schedule.startTime),
    timezone: schedule.timezone,
    window_minutes: schedule.windowMinutes,
    lead_minutes: schedule.leadMinutes,
    trail_minutes: schedule.trailMinutes,
    replay_from_start: schedule.replayFromStart
  } as unknown as StreamScheduleRequest
  const saved = await streams.saveSchedule(body, schedule.id ?? undefined)
  if (saved) Object.assign(schedule, emptySchedule())
}

function editSchedule(entry: StreamSchedule): void {
  schedule.id = entry.id
  schedule.channelId = entry.channel_id
  schedule.name = entry.name
  schedule.days = [...((entry as unknown as { days?: number[] }).days ?? [])]
  schedule.startTime = minuteToTime((entry as unknown as { start_minute?: number }).start_minute ?? 0)
  schedule.timezone = entry.timezone
  schedule.windowMinutes = entry.window_minutes
  schedule.leadMinutes = entry.lead_minutes
  schedule.trailMinutes = entry.trail_minutes
  schedule.replayFromStart = entry.replay_from_start
}

function channelName(id: string): string {
  return channels.value.find(entry => entry.id === id)?.name ?? id
}

/** Runs of one schedule, newest first, bounded to what fits on screen. */
function runsFor(scheduleId: string) {
  return runs.value.filter(run => run.schedule_id === scheduleId).slice(0, 5)
}
</script>

<template>
  <UDashboardPanel id="streams">
    <template #header>
      <UDashboardNavbar :title="t('streams.title')">
        <template #leading><UDashboardSidebarCollapse /></template>
      </UDashboardNavbar>
    </template>
    <template #body>
      <div class="w-full space-y-4">
    <section class="border border-muted bg-default p-5">
      <p class="eyebrow">{{ t('streams.eyebrow') }}</p>
      <div class="mt-1 flex flex-wrap items-center justify-between gap-2">
        <h2 class="text-lg font-semibold text-highlighted">{{ t('streams.title') }}</h2>
        <AreaBackupButtons area="streams" @imported="streams.refresh(); streams.refreshSchedules()" />
      </div>
      <p class="mb-4 mt-1 text-xs leading-5 text-muted">{{ t('streams.description') }}</p>
      <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
      <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />
      <form class="grid gap-3 sm:grid-cols-2" @submit.prevent="submit">
        <UFormField :label="t('streams.form.url_label')" :description="t('streams.form.url_description')" class="sm:col-span-2">
          <UInput v-model="form.url" required class="w-full font-mono" placeholder="https://twitch.tv/channel" icon="i-lucide-radio" />
        </UFormField>
        <UFormField :label="t('streams.form.name_label')" :description="t('streams.form.name_description')">
          <UInput :model-value="form.name ?? ''" class="w-full" :placeholder="t('streams.form.name_placeholder')" @update:model-value="(value: string | number) => (form.name = String(value) || null)" />
        </UFormField>
        <UFormField :label="t('streams.form.quality_label')" :description="t('streams.form.quality_description')">
          <USelect v-model="quality" :items="qualityItems" value-key="value" icon="i-lucide-gauge" class="w-full font-mono" />
        </UFormField>
        <UFormField :label="t('streams.form.category_label')" :description="t('streams.form.category_description')">
          <USelect v-model="category" :items="categoryItems" value-key="value" class="w-full" />
        </UFormField>
        <UFormField :label="t('streams.recording.split')" :description="t('streams.recording.split_hint')">
          <div class="flex gap-2">
            <USelect v-model="form.splitMode" :items="splitItems" value-key="value" class="grow" />
            <UInput
              v-if="form.splitMode !== 'none'"
              v-model.number="form.splitValue"
              type="number"
              min="1"
              class="w-28"
            />
          </div>
        </UFormField>
        <UFormField :label="t('streams.recording.remux')" :description="t('streams.recording.remux_hint')">
          <USelect v-model="form.remux" :items="remuxItems" value-key="value" />
        </UFormField>
        <UFormField :label="t('streams.recording.sidecars')" :description="t('streams.recording.sidecars_hint')">
          <div class="flex flex-wrap gap-4">
            <UCheckbox v-model="form.sidecarMetadata" :label="t('streams.recording.sidecar_metadata')" />
            <UCheckbox v-model="form.sidecarThumbnail" :label="t('streams.recording.sidecar_thumbnail')" />
          </div>
        </UFormField>
        <UFormField :label="t('streams.recording.reconnect')" :description="t('streams.recording.reconnect_hint')">
          <UInput v-model.number="form.reconnectDelay" type="number" min="0" max="600" />
        </UFormField>
        <UFormField :label="t('streams.form.enabled_label')" :description="t('streams.form.enabled_description')">
          <USwitch v-model="form.enabled" :aria-label="t('streams.form.enabled_label')" />
        </UFormField>
        <div class="flex flex-wrap gap-2 sm:col-span-2">
          <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-plus'" :label="editingId ? t('common.actions.save') : t('streams.form.add')" :loading="pending" />
          <UButton type="button" color="neutral" variant="outline" icon="i-lucide-circle-dot" :label="t('streams.form.record_now')" :disabled="!form.url.trim()" @click="recordNow(form.url, form.name, form.quality, form.category_id)" />
          <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('common.actions.cancel')" @click="list.reset" />
        </div>
      </form>
    </section>

    <section class="border border-muted bg-default p-5">
      <div class="mb-3 flex items-center justify-between">
        <h3 class="text-base font-semibold text-highlighted">{{ t('streams.channels_title') }}</h3>
        <UBadge color="neutral" variant="outline">{{ channels.length }}</UBadge>
      </div>
      <div class="grid gap-2">
        <div v-for="channel in channels" :key="channel.id" class="border border-muted p-3">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-radio" class="size-4 shrink-0 text-primary" />
            <p class="min-w-0 flex-1 truncate text-sm font-medium text-highlighted">{{ channel.name }}</p>
            <UBadge size="sm" color="neutral" variant="subtle" class="font-mono">{{ channel.quality ?? t('streams.quality_default') }}</UBadge>
            <USwitch :model-value="channel.enabled" :aria-label="t('streams.form.enabled_label')" @update:model-value="(value: boolean) => toggle(channel, value)" />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-circle-dot" :aria-label="t('streams.form.record_now')" @click="recordNow(channel.url, channel.name, channel.quality, channel.category_id)" />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" @click="edit(channel)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" @click="remove(channel)" />
          </div>
          <p class="mt-1 truncate font-mono text-[11px] text-muted">{{ channel.url }}</p>
          <div class="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted">
            <span v-if="channel.last_live_at">{{ t('streams.last_live', { time: formatMoment(channel.last_live_at) }) }}</span>
            <span v-else>{{ t('streams.never_live') }}</span>
            <span v-if="channel.last_error" class="text-warning" :title="channel.last_error">{{ t('streams.probe_error') }}</span>
          </div>
        </div>
        <p v-if="!channels.length && !loading" class="border border-dashed border-muted p-5 text-center text-sm text-muted">{{ t('streams.empty') }}</p>
      </div>
    </section>

    <section v-if="!loading" class="flex flex-col gap-3">
      <h2 class="text-sm font-semibold">{{ t('streams.schedules.title') }}</h2>
      <p class="text-xs text-muted">{{ t('streams.schedules.hint') }}</p>

      <!--
        A schedule needs a channel to record. Without one the picker is empty, the submit
        button is still live, and posting sends an empty channel id that the server rejects —
        so the form is replaced by the one instruction that gets you out of the situation.
        `!loading` on the section, as on the channel empty state above, so nothing flashes on
        the first render.
      -->
      <p
        v-if="!channels.length"
        class="border border-dashed border-muted p-5 text-center text-sm text-muted"
      >
        {{ t('streams.schedules.needs_channel') }}
      </p>

      <template v-else>
      <form class="grid gap-3 sm:grid-cols-2" @submit.prevent="submitSchedule">
        <UFormField :label="t('streams.schedules.channel')">
          <USelect v-model="schedule.channelId" :items="channelItems" value-key="value" />
        </UFormField>
        <UFormField :label="t('streams.schedules.name')">
          <UInput v-model="schedule.name" required data-testid="schedule-name" />
        </UFormField>
        <UFormField :label="t('streams.schedules.days')" class="sm:col-span-2">
          <div class="flex flex-wrap gap-1">
            <UButton
              v-for="day in WEEKDAYS"
              :key="day"
              size="xs"
              :variant="schedule.days.includes(day) ? 'solid' : 'outline'"
              @click="toggleDay(day)"
            >
              {{ t(`streams.schedules.weekday.${day}`) }}
            </UButton>
          </div>
        </UFormField>
        <UFormField :label="t('streams.schedules.start')">
          <UInput v-model="schedule.startTime" type="time" required />
        </UFormField>
        <UFormField :label="t('streams.schedules.timezone')" :description="t('streams.schedules.timezone_hint')">
          <TimezoneSelect v-model="schedule.timezone" :aria-label="t('streams.schedules.timezone')" />
        </UFormField>
        <UFormField :label="t('streams.schedules.window')">
          <UInput v-model.number="schedule.windowMinutes" type="number" min="1" />
        </UFormField>
        <UFormField :label="t('streams.schedules.lead')" :description="t('streams.schedules.roll_hint')">
          <UInput v-model.number="schedule.leadMinutes" type="number" min="0" max="120" />
        </UFormField>
        <UFormField :label="t('streams.schedules.trail')">
          <UInput v-model.number="schedule.trailMinutes" type="number" min="0" max="120" />
        </UFormField>
        <UFormField :label="t('streams.schedules.replay')" :description="t('streams.schedules.replay_hint')">
          <USwitch v-model="schedule.replayFromStart" />
        </UFormField>
        <div class="flex gap-2 sm:col-span-2">
          <UButton type="submit" data-testid="schedule-submit">
            {{ schedule.id ? t('common.actions.save') : t('streams.schedules.add') }}
          </UButton>
          <UButton
            v-if="schedule.id"
            color="neutral"
            variant="ghost"
            @click="Object.assign(schedule, emptySchedule())"
          >
            {{ t('common.actions.cancel') }}
          </UButton>
        </div>
      </form>

      <p v-if="!schedules.length" class="border border-dashed border-muted p-5 text-center text-sm text-muted">
        {{ t('streams.schedules.empty') }}
      </p>
      <div v-for="entry in schedules" :key="entry.id" class="border border-muted bg-default p-4">
        <div class="flex flex-wrap items-center gap-2">
          <span class="font-medium">{{ entry.name }}</span>
          <UBadge color="neutral" variant="subtle">{{ channelName(entry.channel_id) }}</UBadge>
          <UBadge v-if="entry.replay_from_start" color="neutral" variant="subtle">
            {{ t('streams.schedules.replay') }}
          </UBadge>
          <span class="text-xs text-muted">{{ entry.timezone }}</span>
          <span class="grow" />
          <UButton size="xs" variant="ghost" @click="editSchedule(entry)">{{ t('common.actions.edit') }}</UButton>
          <UButton size="xs" color="error" variant="ghost" @click="streams.removeSchedule(entry.id)">
            {{ t('common.actions.delete') }}
          </UButton>
        </div>
        <ul class="mt-2 flex flex-col gap-1 text-xs text-muted">
          <li v-for="run in runsFor(entry.id)" :key="run.id" class="flex flex-wrap items-center gap-2">
            <UBadge
              :color="run.state === 'missed' || run.state === 'failed' ? 'warning' : 'neutral'"
              variant="subtle"
            >
              {{ t(`streams.schedules.states.${run.state}`) }}
            </UBadge>
            <span>{{ formatMoment(run.starts_at) }}</span>
            <span v-if="run.replay_used">{{ t('streams.schedules.replay_used') }}</span>
            <span v-if="run.error" class="text-error">{{ run.error }}</span>
          </li>
        </ul>
      </div>
      </template>
    </section>
      </div>
    </template>
  </UDashboardPanel>
</template>
