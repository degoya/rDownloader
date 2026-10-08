<script setup lang="ts">
/**
 * What an account whose traffic its hoster reports used up does to the queue (RD-1190-14): the
 * default, and a different choice per account. Part of the settings document, saved with the
 * rest of the page.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Account, AccountTrafficAction, Settings } from '@/api/types'
import { useQueuePauseStore } from '@/stores/queuePause'
import { formatPauseEnd } from '@/utils/format'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const queuePause = useQueuePauseStore()
const accounts = ref<Account[]>([])

/** The select's stand-in for "no override": a select item cannot carry an empty value. */
const INHERIT = 'inherit'
const ACTIONS: AccountTrafficAction[] = ['nothing', 'pause_account', 'pause_queue']

const actionItems = computed(() => ACTIONS.map(value => ({ value, label: t(`settings.account_traffic.${value}`) })))
const overrideItems = computed(() => [{ value: INHERIT, label: t('settings.account_traffic.inherit') }, ...actionItems.value])

function overrideOf(id: string): string {
  return settings.value.account_traffic_overrides[id] ?? INHERIT
}

function setOverride(id: string, value: string): void {
  const next = { ...settings.value.account_traffic_overrides }
  if (value === INHERIT) delete next[id]
  else next[id] = value as AccountTrafficAction
  settings.value.account_traffic_overrides = next
}

function heldUntil(id: string): string | null {
  const hold = queuePause.accountTraffic.find(item => item.account_id === id)
  return hold ? formatPauseEnd(hold.next_check_at) : null
}

onMounted(async () => {
  try {
    const response = await api.GET('/api/v1/accounts')
    accounts.value = response.data ?? []
  } catch {
    // Without the list only the default can be chosen; the overrides already stored stay.
  }
})
</script>

<template>
  <div class="grid gap-4" data-testid="account-traffic-settings">
    <UFormField data-settings-anchor="general.account_traffic" :label="t('settings.account_traffic.label')" :description="t('settings.account_traffic.description')">
      <USelect v-model="settings.account_traffic_action" :items="actionItems" value-key="value" class="mt-2 w-full" data-testid="account-traffic-action" />
    </UFormField>
    <UFormField v-if="accounts.length > 0" :label="t('settings.account_traffic.overrides_label')" :description="t('settings.account_traffic.overrides_description')">
      <div class="mt-2 grid gap-2">
        <div v-for="account in accounts" :key="account.id" class="flex flex-wrap items-center gap-x-3 gap-y-1">
          <div class="min-w-0 flex-1 basis-40">
            <p class="truncate text-sm text-highlighted">{{ account.label }}</p>
            <p class="truncate font-mono text-2xs text-muted">{{ account.provider }}</p>
          </div>
          <UBadge v-if="heldUntil(account.id)" size="sm" color="warning" variant="subtle" icon="i-lucide-gauge" :label="t('settings.account_traffic.held', { time: heldUntil(account.id) })" />
          <USelect
            :model-value="overrideOf(account.id)"
            :items="overrideItems"
            value-key="value"
            class="w-64 max-w-full"
            :aria-label="account.label"
            :data-testid="`account-traffic-override-${account.id}`"
            @update:model-value="(value: string) => setOverride(account.id, value)"
          />
        </div>
      </div>
    </UFormField>
  </div>
</template>
