import { storeToRefs } from 'pinia'
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { StreamSchedule, StreamScheduleRequest } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import { useCopyName } from '@/composables/useCopyName'
import { useFormFocus } from '@/composables/useFormFocus'
import { useStreamsStore } from '@/stores/streams'
import { browserTimezone } from '@/utils/timezones'

const WEEKDAYS = [1, 2, 3, 4, 5, 6, 7] as const
/** Matches `MAX_NAME` in `crates/rd-api-intake/src/stream_schedule_handlers.rs`. */
const MAX_SCHEDULE_NAME = 200

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

/** The stream schedules of `StreamsView.vue` (RD-080-08): their form, list actions and runs. */
export function useStreamSchedules() {
  const { t } = useI18n()
  const streams = useStreamsStore()
  const { channels, schedules, runs } = storeToRefs(streams)
  const confirm = useConfirm()
  const copyName = useCopyName()

  const schedule = reactive<ScheduleForm>(emptySchedule())
  const scheduleForm = ref<HTMLFormElement | null>(null)
  const focusScheduleForm = useFormFocus(scheduleForm)
  /** The failure of the last schedule save, shown above the schedule form rather than the page. */
  const scheduleError = ref<string | null>(null)
  const duplicatingScheduleId = ref<string | null>(null)

  const channelItems = computed(() =>
    channels.value.map(entry => ({ value: entry.id, label: entry.name }))
  )

  const weekdayItems = computed(() => WEEKDAYS.map(day => ({ value: day, label: t(`streams.schedules.weekday.${day}`) })))
  /** Kept in weekday order whatever order the boxes were ticked in. */
  const scheduleDays = computed({
    get: () => schedule.days,
    set: (days: number[]) => { schedule.days = [...days].sort((a, b) => a - b) }
  })

  /** A schedule is deleted after the same confirmation as a channel (design.md, destructive actions). */
  async function removeSchedule(entry: StreamSchedule): Promise<void> {
    const confirmed = await confirm({
      title: t('streams.schedules.delete.title'),
      description: t('streams.schedules.delete.description', { name: entry.name }),
      confirmLabel: t('common.actions.delete'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (confirmed) await streams.removeSchedule(entry.id)
  }

  function resetSchedule(): void {
    scheduleError.value = null
    Object.assign(schedule, emptySchedule())
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
    scheduleError.value = null
    const saved = await streams.saveSchedule(body, schedule.id ?? undefined)
    if (saved) resetSchedule()
    else scheduleError.value = streams.error
  }

  /**
   * Copies a schedule — channel, days, times, margins — under a free name and opens the copy for
   * editing, so "the same channel at another time" is one changed field (RD-150-12). The runs
   * recorded so far stay with the original.
   */
  async function duplicateSchedule(entry: StreamSchedule): Promise<void> {
    const source = entry as unknown as StreamSchedule & { days?: number[], start_minute?: number }
    duplicatingScheduleId.value = entry.id
    scheduleError.value = null
    const body = {
      channel_id: entry.channel_id,
      name: copyName(entry.name, schedules.value.map(item => item.name), MAX_SCHEDULE_NAME),
      enabled: entry.enabled,
      kind: 'weekly',
      days: [...(source.days ?? [])],
      start_minute: source.start_minute ?? 0,
      timezone: entry.timezone,
      window_minutes: entry.window_minutes,
      lead_minutes: entry.lead_minutes,
      trail_minutes: entry.trail_minutes,
      replay_from_start: entry.replay_from_start
    } as unknown as StreamScheduleRequest
    const saved = await streams.saveSchedule(body)
    duplicatingScheduleId.value = null
    if (!saved) return void (scheduleError.value = streams.error)
    editSchedule(saved)
  }

  function editSchedule(entry: StreamSchedule): void {
    scheduleError.value = null
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
    void focusScheduleForm()
  }

  function channelName(id: string): string {
    return channels.value.find(entry => entry.id === id)?.name ?? id
  }

  /** Runs of one schedule, newest first, bounded to what fits on screen. */
  function runsFor(scheduleId: string) {
    return runs.value.filter(run => run.schedule_id === scheduleId).slice(0, 5)
  }

  return {
    schedule,
    scheduleForm,
    scheduleError,
    duplicatingScheduleId,
    channelItems,
    weekdayItems,
    scheduleDays,
    removeSchedule,
    resetSchedule,
    submitSchedule,
    duplicateSchedule,
    editSchedule,
    channelName,
    runsFor
  }
}
