<script setup lang="ts">
/**
 * One shortcut of the capture agent's tray commands (RD-1180-03): recorded by pressing it, not
 * typed. "Record" listens on its own button for the next combination; Esc stops listening
 * without a change, and Tab leaves the field as it always does, so the keyboard is never caught.
 * "Reset" brings the built-in combination back, "No shortcut" clears it.
 *
 * What may be registered is the service's decision; a refusal arrives as `error` and stands
 * under the field it is about.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { formatShortcut, shortcutFromKeyboardEvent, type ShortcutPlatform } from '@/utils/captureShortcuts'

const model = defineModel<string | null>({ required: true })
const props = defineProps<{
  label: string
  /** The built-in combination, `null` where there is none (Quit). */
  defaultValue: string | null
  platform: ShortcutPlatform
  error?: string | null | undefined
}>()
const { t } = useI18n()
const recording = ref(false)

const keys = computed(() => (model.value ? formatShortcut(model.value, props.platform) : []))

function toggleRecording(): void {
  recording.value = !recording.value
}

function onKeydown(event: KeyboardEvent): void {
  if (!recording.value) return
  const modified = event.ctrlKey || event.altKey || event.metaKey
  if (!modified && event.code === 'Tab') {
    recording.value = false
    return
  }
  event.preventDefault()
  event.stopPropagation()
  if (!modified && !event.shiftKey && event.code === 'Escape') {
    recording.value = false
    return
  }
  const shortcut = shortcutFromKeyboardEvent(event)
  if (!shortcut) return
  model.value = shortcut
  recording.value = false
}
</script>

<template>
  <UFormField :label="props.label" :error="props.error || undefined">
    <div class="flex flex-wrap items-center gap-2">
      <span class="flex min-w-36 items-center gap-1" data-testid="shortcut-keys">
        <template v-if="keys.length">
          <UKbd v-for="key in keys" :key="key" :value="key" />
        </template>
        <span v-else class="text-sm text-muted">{{ t('settings.capture_agent.shortcuts.none_set') }}</span>
      </span>
      <UButton
        :label="recording ? t('settings.capture_agent.shortcuts.recording') : t('settings.capture_agent.shortcuts.record')"
        :aria-label="`${t('settings.capture_agent.shortcuts.record')}: ${props.label}`"
        :aria-pressed="recording"
        icon="i-lucide-keyboard"
        color="neutral"
        :variant="recording ? 'solid' : 'outline'"
        size="sm"
        data-testid="shortcut-record"
        @click="toggleRecording"
        @keydown="onKeydown"
        @blur="recording = false"
      />
      <UButton
        :label="t('settings.capture_agent.shortcuts.reset')"
        :aria-label="`${t('settings.capture_agent.shortcuts.reset')}: ${props.label}`"
        icon="i-lucide-rotate-ccw"
        color="neutral"
        variant="ghost"
        size="sm"
        :disabled="model === props.defaultValue"
        data-testid="shortcut-reset"
        @click="model = props.defaultValue"
      />
      <UButton
        :label="t('settings.capture_agent.shortcuts.none')"
        :aria-label="`${t('settings.capture_agent.shortcuts.none')}: ${props.label}`"
        icon="i-lucide-x"
        color="neutral"
        variant="ghost"
        size="sm"
        :disabled="model === null"
        data-testid="shortcut-none"
        @click="model = null"
      />
    </div>
    <span class="sr-only" aria-live="polite">{{ recording ? t('settings.capture_agent.shortcuts.recording_hint') : '' }}</span>
  </UFormField>
</template>
