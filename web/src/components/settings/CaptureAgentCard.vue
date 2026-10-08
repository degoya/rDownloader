<script setup lang="ts">
/**
 * What the desktop capture agent does on its computer, set from here (RD-1180-01, RD-1180-03):
 * whether it watches the clipboard, and the system-wide shortcuts of its tray commands.
 *
 * The pause is a switch that saves at once, like the tray's own; the shortcuts are edited as a
 * set and saved together, because two of them can only be judged against each other. Both are
 * one row of the service that the tray switches too, so the card reloads when the service says
 * it changed (`capture.changed`) — and keeps shortcuts somebody is still editing.
 */
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureAgentSettingsResponse, CaptureCommand, CaptureShortcuts } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import ShortcutField from '@/components/settings/ShortcutField.vue'
import { subscribeEvents } from '@/composables/useEventStream'
import { useFetchState } from '@/composables/useFetchState'
import { CAPTURE_COMMANDS, duplicateCommands, guessPlatform } from '@/utils/captureShortcuts'
import { formatMoment } from '@/utils/format'
import { isRecord } from '@/utils/values'

const { t } = useI18n()
const current = ref<CaptureAgentSettingsResponse | null>(null)
const draft = reactive<CaptureShortcuts>({})
const fieldErrors = ref<Partial<Record<CaptureCommand, string>>>({})
const pausing = ref(false)
const saving = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const { loading, loadError, load: trackLoad } = useFetchState()

const platform = computed(() => current.value?.report?.platform ?? guessPlatform())
const paused = computed(() => current.value?.clipboard_paused ?? false)
const dirty = computed(() => current.value !== null
  && CAPTURE_COMMANDS.some(command => (draft[command] ?? null) !== (current.value?.shortcuts[command] ?? null)))
const report = computed(() => current.value?.report ?? null)
const reportText = computed(() => {
  const value = report.value
  if (!value) return null
  const when = t('settings.capture_agent.report.reported_at', { time: formatMoment(value.reported_at) })
  if (value.unavailable) return `${t(`settings.capture_agent.report.unavailable.${value.unavailable}`)} ${when}`
  if (!value.refused?.length) return null
  const commands = value.refused.map(command => commandLabel(command)).join(', ')
  return `${t('settings.capture_agent.report.refused', { commands })} ${when}`
})

function commandLabel(command: CaptureCommand): string {
  return t(`settings.capture_agent.commands.${command}`)
}

function isCommand(value: unknown): value is CaptureCommand {
  return typeof value === 'string' && (CAPTURE_COMMANDS as readonly string[]).includes(value)
}

/** Takes the service's answer; shortcuts still being edited stay as they are. */
function apply(value: CaptureAgentSettingsResponse, keepDraft: boolean): void {
  current.value = value
  if (keepDraft) return
  for (const command of CAPTURE_COMMANDS) draft[command] = value.shortcuts[command] ?? null
  fieldErrors.value = {}
}

async function load(): Promise<void> {
  await trackLoad(async () => {
    const response = await api.GET('/api/v1/settings/capture-agent')
    if (!response.data) return responseError(response)
    apply(response.data, dirty.value)
    return null
  })
}

let release: (() => void) | null = null
onMounted(() => {
  void load()
  release = subscribeEvents({
    'capture.changed': (event: MessageEvent<string>) => {
      try {
        const envelope: unknown = JSON.parse(event.data)
        const payload = isRecord(envelope) && isRecord(envelope.payload) ? envelope.payload : null
        // The same event announces paired agents coming and going; those are not this card's.
        if (payload && payload.resource !== 'capture_agent_settings') return
      } catch {
        // An envelope that does not read is no reason not to look again.
      }
      void load()
    }
  })
})
onUnmounted(() => release?.())

async function setPaused(value: boolean): Promise<void> {
  pausing.value = true
  error.value = null
  message.value = null
  const response = await api.PATCH('/api/v1/settings/capture-agent', { body: { clipboard_paused: value } })
  pausing.value = false
  if (response.data) apply(response.data, dirty.value)
  else error.value = responseError(response)
}

/** Two commands with one combination are named before the service refuses the save for it. */
function validate(): { name: string, message: string }[] {
  const duplicates = [...duplicateCommands(draft)].map(([command, other]) => ({
    name: command,
    message: t('settings.capture_agent.shortcuts.duplicate', { other: commandLabel(other) })
  }))
  fieldErrors.value = Object.fromEntries(duplicates.map(entry => [entry.name, entry.message]))
  return duplicates
}

function discard(): void {
  if (current.value) apply(current.value, false)
}

async function saveShortcuts(): Promise<void> {
  error.value = null
  message.value = null
  saving.value = true
  const shortcuts = Object.fromEntries(CAPTURE_COMMANDS.map(command => [command, draft[command] ?? null])) as CaptureShortcuts
  const response = await api.PATCH('/api/v1/settings/capture-agent', { body: { shortcuts } })
  saving.value = false
  if (response.data) {
    apply(response.data, false)
    message.value = t('settings.capture_agent.shortcuts.saved')
    return
  }
  // A refusal names the command it is about; it stands under that field, not at the top.
  const body: unknown = response.error
  const params = isRecord(body) && isRecord(body.params) ? body.params : null
  if (!isCommand(params?.command)) {
    error.value = responseError(response)
    return
  }
  fieldErrors.value = {
    [params.command]: isCommand(params.other)
      ? t('settings.capture_agent.shortcuts.duplicate', { other: commandLabel(params.other) })
      : responseError(response)
  }
}
</script>

<template>
  <div class="space-y-5" data-testid="capture-agent-card">
    <SectionHeader
      :eyebrow="t('settings.capture_agent.eyebrow')"
      :title="t('settings.capture_agent.title')"
      :description="t('settings.capture_agent.description')"
    />
    <DataState v-if="!current" :loading="loading" :error="loadError" :rows="4" />
    <template v-else>
      <UAlert v-if="error" color="error" icon="i-lucide-circle-alert" :description="error" />
      <div class="flex flex-wrap items-start justify-between gap-3">
        <USwitch
          class="max-w-prose"
          :model-value="paused"
          :label="t('settings.capture_agent.clipboard_paused.label')"
          :description="t('settings.capture_agent.clipboard_paused.description')"
          :disabled="pausing"
          data-testid="clipboard-paused"
          @update:model-value="setPaused"
        />
        <UBadge
          :color="paused ? 'warning' : 'success'"
          variant="subtle"
          :icon="paused ? 'i-lucide-clipboard-x' : 'i-lucide-clipboard-check'"
          :label="paused ? t('settings.capture_agent.paused_badge') : t('settings.capture_agent.watching_badge')"
          data-testid="clipboard-state"
        />
      </div>
      <USeparator />
      <SectionHeader
        :eyebrow="t('settings.capture_agent.shortcuts.eyebrow')"
        :title="t('settings.capture_agent.shortcuts.title')"
        :description="t('settings.capture_agent.shortcuts.description')"
        level="sub"
      />
      <UAlert
        v-if="reportText"
        color="warning"
        icon="i-lucide-keyboard-off"
        :title="report?.unavailable ? t('settings.capture_agent.report.title_unavailable') : t('settings.capture_agent.report.title_refused')"
        :description="reportText"
        data-testid="shortcut-report"
      />
      <UAlert v-if="message" color="success" icon="i-lucide-circle-check" :description="message" data-testid="shortcuts-saved" />
      <UForm :state="draft" :validate="validate" class="space-y-4" data-testid="shortcuts-form" @submit="saveShortcuts">
        <ShortcutField
          v-for="command in CAPTURE_COMMANDS"
          :key="command"
          :model-value="draft[command] ?? null"
          :label="commandLabel(command)"
          :default-value="current.default_shortcuts[command] ?? null"
          :platform="platform"
          :error="fieldErrors[command]"
          :data-testid="`shortcut-${command}`"
          @update:model-value="value => (draft[command] = value)"
        />
        <FormActions
          editing
          :cancellable="dirty"
          :create-label="t('settings.capture_agent.shortcuts.save')"
          :save-label="t('settings.capture_agent.shortcuts.save')"
          :loading="saving"
          :disabled="!dirty"
          @cancel="discard"
        />
      </UForm>
    </template>
  </div>
</template>
