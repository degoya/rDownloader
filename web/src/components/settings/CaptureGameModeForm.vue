<script setup lang="ts">
/**
 * The desktop agent's game mode (RD-1240-19): while a full-screen program is in front or a named
 * program runs, the agent pauses the queue or switches on a bandwidth profile, and lifts only
 * what it set itself once that is over.
 *
 * Edited as a whole and saved with its own button, like the shortcuts beside it; a reload the
 * service announces keeps what somebody is still editing. The on/off switch is the same one the
 * tray's "Pause while gaming" flips (RD-1240-23): a switch made there shows here once the page
 * reloads, unless the form is being edited.
 */
import { computed, nextTick, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthProfile, CaptureAgentSettingsResponse, CaptureGameMode } from '@/api/types'
import FormActions from '@/components/FormActions.vue'
import SearchableSelect from '@/components/SearchableSelect.vue'
import SectionHeader from '@/components/SectionHeader.vue'

const props = defineProps<{ gameMode: CaptureGameMode }>()
const emit = defineEmits<{ saved: [value: CaptureAgentSettingsResponse] }>()

const { t } = useI18n()
const draft = reactive<Required<CaptureGameMode>>({ enabled: true, full_screen: false, processes: [], action: 'pause', profile_id: null })
const profiles = ref<BandwidthProfile[]>([])
const saving = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)

const actionItems = computed(() => [
  { value: 'pause' as const, label: t('settings.capture_agent.game_mode.action.pause'), description: t('settings.capture_agent.game_mode.action.pause_description') },
  { value: 'profile' as const, label: t('settings.capture_agent.game_mode.action.profile'), description: t('settings.capture_agent.game_mode.action.profile_description') }
])
const profileItems = computed(() => profiles.value.map(profile => ({ value: profile.id, label: profile.name })))

function stored(): Required<CaptureGameMode> {
  return {
    // Settings stored before the switch existed are on.
    enabled: props.gameMode.enabled ?? true,
    full_screen: props.gameMode.full_screen ?? false,
    processes: [...(props.gameMode.processes ?? [])],
    action: props.gameMode.action ?? 'pause',
    profile_id: props.gameMode.profile_id ?? null
  }
}

const dirty = computed(() => JSON.stringify(draft) !== JSON.stringify(stored()))

function reset(): void {
  Object.assign(draft, stored())
}

reset()
watch(() => props.gameMode, () => {
  if (!dirty.value) reset()
})

onMounted(async () => {
  const response = await api.GET('/api/v1/bandwidth/profiles')
  if (response.data) profiles.value = response.data
})

function validate(): { name: string, message: string }[] {
  if (draft.action === 'profile' && !draft.profile_id) {
    return [{ name: 'profile_id', message: t('settings.capture_agent.game_mode.profile_required') }]
  }
  return []
}

async function save(): Promise<void> {
  error.value = null
  message.value = null
  saving.value = true
  const game_mode: CaptureGameMode = { ...draft, profile_id: draft.action === 'profile' ? draft.profile_id : null }
  const response = await api.PATCH('/api/v1/settings/capture-agent', { body: { game_mode } })
  saving.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  emit('saved', response.data)
  // The stored form, names trimmed and once each, once the card has passed it down.
  await nextTick()
  reset()
  message.value = t('settings.capture_agent.game_mode.saved')
}
</script>

<template>
  <div class="space-y-4" data-testid="game-mode">
    <SectionHeader
      :eyebrow="t('settings.capture_agent.game_mode.eyebrow')"
      :title="t('settings.capture_agent.game_mode.title')"
      :description="t('settings.capture_agent.game_mode.description')"
      level="sub"
    />
    <UAlert v-if="error" color="error" icon="i-lucide-circle-alert" :description="error" data-testid="game-mode-error" />
    <UAlert v-if="message" color="success" icon="i-lucide-circle-check" :description="message" data-testid="game-mode-saved" />
    <UForm :state="draft" :validate="validate" class="space-y-4" data-testid="game-mode-form" @submit="save">
      <USwitch
        v-model="draft.enabled"
        class="max-w-prose"
        :label="t('settings.capture_agent.game_mode.enabled.label')"
        :description="t('settings.capture_agent.game_mode.enabled.description')"
        data-testid="game-mode-enabled"
      />
      <USwitch
        v-model="draft.full_screen"
        class="max-w-prose"
        :label="t('settings.capture_agent.game_mode.full_screen.label')"
        :description="t('settings.capture_agent.game_mode.full_screen.description')"
        data-testid="game-mode-full-screen"
      />
      <UFormField
        name="processes"
        :label="t('settings.capture_agent.game_mode.processes.label')"
        :description="t('settings.capture_agent.game_mode.processes.description')"
      >
        <UInputTags
          v-model="draft.processes"
          :placeholder="t('settings.capture_agent.game_mode.processes.placeholder')"
          icon="i-lucide-gamepad-2"
          add-on-blur
          add-on-paste
          delimiter=","
          class="w-full font-mono"
          data-testid="game-mode-processes"
        />
      </UFormField>
      <URadioGroup
        v-model="draft.action"
        :legend="t('settings.capture_agent.game_mode.action.legend')"
        :items="actionItems"
        data-testid="game-mode-action"
      />
      <UFormField
        v-if="draft.action === 'profile'"
        name="profile_id"
        :label="t('settings.capture_agent.game_mode.profile.label')"
        :description="profileItems.length ? undefined : t('settings.capture_agent.game_mode.profile.none')"
      >
        <SearchableSelect
          :model-value="draft.profile_id ?? undefined"
          :items="profileItems"
          :placeholder="t('settings.capture_agent.game_mode.profile.placeholder')"
          class="w-full"
          data-testid="game-mode-profile"
          @update:model-value="(id: string) => { draft.profile_id = id }"
        />
      </UFormField>
      <FormActions
        editing
        :cancellable="dirty"
        :create-label="t('settings.capture_agent.game_mode.save')"
        :save-label="t('settings.capture_agent.game_mode.save')"
        :loading="saving"
        :disabled="!dirty"
        @cancel="reset"
      />
    </UForm>
  </div>
</template>
