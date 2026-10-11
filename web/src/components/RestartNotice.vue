<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useRestartRefresh, useRestartStatus } from '@/composables/useRestartStatus'

/**
 * The sidebar's line that rDownloader waits for a restart (RD-1240-32), beside the update
 * notice: nothing while none is pending, "Restart pending" when one is, "Restarting …" while it
 * runs, and a click leads to the Updates page, where the reasons and "Restart now" are. This is
 * the component that is always mounted, so it is the one that keeps the status current.
 */
defineProps<{ collapsed?: boolean }>()
const { t } = useI18n()
const { status, restarting } = useRestartStatus()
useRestartRefresh()

const going = computed(() => restarting.value || status.value?.restarting === true)
const label = computed(() => t(going.value ? 'system.restart.restarting_title' : 'system.restart.title'))
</script>

<template>
  <div v-if="going || status?.pending" data-testid="restart-notice" :class="collapsed ? 'flex justify-center py-1' : 'pb-2'">
    <UTooltip :text="label" :disabled="!collapsed">
      <UButton
        :icon="going ? 'i-lucide-loader-circle' : 'i-lucide-rotate-ccw'"
        size="xs"
        color="warning"
        variant="soft"
        to="/settings/system?tab=updates"
        :class="collapsed ? undefined : 'w-full'"
        :ui="going ? { leadingIcon: 'animate-spin' } : undefined"
        :label="collapsed ? undefined : label"
        :aria-label="label"
      />
    </UTooltip>
  </div>
</template>
