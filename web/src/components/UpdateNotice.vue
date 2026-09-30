<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { INSTALL_ENDED } from '@/api/updates'
import UpdateDetailsModal from '@/components/UpdateDetailsModal.vue'
import { useUpdateStatus } from '@/composables/useUpdateStatus'

/**
 * The sidebar's quiet hint that a newer version is out (RD-180-01): nothing at all while there
 * is none, one line above the preferences when there is, and the details a click away. Read
 * once on mount and every few hours after, which is as often as the service checks at most.
 */
defineProps<{ collapsed?: boolean }>()
const { t } = useI18n()
const { status, followed, load } = useUpdateStatus()
const detailsOpen = ref(false)
/**
 * An install this page followed keeps the line, and with it the details, also once it ended in
 * the new version and nothing is offered any more.
 */
const tracked = computed(() => followed.value ? status.value?.install ?? null : null)
const running = computed(() => tracked.value !== null && !INSTALL_ENDED.includes(tracked.value.state))
const icon = computed(() => {
  if (running.value) return 'i-lucide-loader-circle'
  return tracked.value?.state === 'done' ? 'i-lucide-circle-check' : 'i-lucide-sparkles'
})
const label = computed(() => {
  const install = tracked.value
  if (install && running.value) return t('system.updates.notice_installing', { version: install.target_version })
  if (install?.state === 'done') return t('system.updates.notice_updated', { version: install.target_version })
  return t('system.updates.notice_version', { version: status.value?.available?.version ?? install?.target_version ?? '' })
})
const REFRESH_MS = 3 * 60 * 60 * 1000
let timer: ReturnType<typeof setInterval> | undefined

onMounted(() => {
  void load()
  timer = setInterval(() => { void load() }, REFRESH_MS)
})
onBeforeUnmount(() => {
  if (timer) clearInterval(timer)
})
</script>

<template>
  <div v-if="status?.available || tracked" data-testid="update-notice" :class="collapsed ? 'flex justify-center py-1' : 'pb-2'">
    <UTooltip :text="label" :disabled="!collapsed">
      <UButton
        :icon="icon"
        size="xs"
        color="primary"
        variant="soft"
        :class="collapsed ? undefined : 'w-full'"
        :ui="running ? { leadingIcon: 'animate-spin' } : undefined"
        :label="collapsed ? undefined : label"
        :aria-label="label"
        @click="detailsOpen = true"
      />
    </UTooltip>
    <!-- Outside the line above: an install that ends in the new version offers nothing any more,
         and its outcome must stay on screen. -->
    <UpdateDetailsModal v-if="status" v-model:open="detailsOpen" :offer="status.available" :kind="status.install_kind" />
  </div>
</template>
