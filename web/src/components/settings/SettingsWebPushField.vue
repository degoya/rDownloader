<script setup lang="ts">
/**
 * "Push on this device" (RD-1240-13): Web Push for this browser only, under the per-browser
 * card of Settings › Interface. Off a secure context the switch stays off and says why.
 */
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NotificationEvent } from '@/api/types'
import { NOTIFICATION_EVENTS } from '@/components/notifications/notificationEvents'
import { useWebPush } from '@/composables/useWebPush'
import { translateServerMessage } from '@/i18n/server'

const { t } = useI18n()
const push = useWebPush()
const { state, busy, error, events } = push

/** Why the switch cannot be turned on here, if it cannot. */
const blocked = computed(() => {
  switch (state.value) {
    case 'insecure': return t('settings.web_push.insecure')
    case 'unsupported': return t('settings.web_push.unsupported')
    case 'denied': return t('settings.notifications.denied')
    default: return null
  }
})

const errorText = computed(() => error.value === null ? null : translateServerMessage(error.value))

const eventItems = computed(() => NOTIFICATION_EVENTS.map(event => ({
  value: event,
  label: t(`notifications.event.${event}`)
})))

function toggle(value: boolean): void {
  void (value ? push.enable() : push.disable())
}

function choose(value: NotificationEvent[]): void {
  void push.setEvents(value)
}

onMounted(() => void push.refresh())
</script>

<template>
  <div class="mt-4 space-y-3">
    <USeparator />
    <UFormField data-settings-anchor="interface.web_push" :label="t('settings.web_push.label')" orientation="horizontal">
      <template #description>
        {{ t('settings.web_push.description') }}
        <span v-if="blocked" class="mt-1 block text-warning" data-testid="web-push-blocked">{{ blocked }}</span>
        <span v-if="errorText" class="mt-1 block text-error" role="alert">{{ errorText }}</span>
      </template>
      <USwitch
        :model-value="state === 'on'"
        :loading="busy"
        :disabled="busy || state === 'insecure' || state === 'unsupported'"
        data-testid="web-push-switch"
        @update:model-value="toggle"
      />
    </UFormField>
    <UFormField v-if="state === 'on'" :label="t('settings.web_push.events_label')" :description="t('settings.web_push.events_description')">
      <USelectMenu
        :model-value="events"
        :items="eventItems"
        value-key="value"
        multiple
        :placeholder="t('notifications.rule.all_events')"
        :disabled="busy"
        class="w-full"
        data-testid="web-push-events"
        @update:model-value="choose"
      />
    </UFormField>
  </div>
</template>
