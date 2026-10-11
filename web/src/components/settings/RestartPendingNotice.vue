<script setup lang="ts">
/**
 * "Restart pending" at the top of Settings > System > Updates (RD-1240-32): which plugin changes
 * wait for the next start, how the restart happens on this installation, why it cannot run right
 * now, and "Restart now". While a restart this page asked for runs, the same place says so, and
 * that the page reloads once the service is back — or that it did not come back.
 */
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'

import type { RestartReason } from '@/api/restart'
import { useRestartAction, useRestartStatus } from '@/composables/useRestartStatus'
import { translateServerMessage } from '@/i18n/server'

const { t, te } = useI18n()
const { status, starting, restarting, lost, load } = useRestartStatus()
const { restartNow } = useRestartAction()

onMounted(() => { void load() })

const visible = computed(() => restarting.value || lost.value || status.value?.restarting === true || status.value?.pending === true)
const going = computed(() => restarting.value || status.value?.restarting === true)

/** One line per reason; a code this page does not know yet still says that plugins changed. */
function reasonText(reason: RestartReason): string {
  const name = reason.name ?? reason.plugin_id ?? t('system.restart.reason.unnamed')
  const plugin = reason.version ? `${name} ${reason.version}` : name
  const key = `system.restart.reason.${reason.code}`
  return te(key) ? t(key, { name, plugin, version: reason.version ?? '', from: reason.from_version ?? '' }) : t('system.restart.reason.other')
}

const reasons = computed(() => (status.value?.reasons ?? []).map(reasonText))

/** How the restart happens here: by itself, by systemd or the container, or by the person. */
const how = computed(() => {
  const current = status.value
  if (!current) return null
  if (current.how === 'supervisor') return t(`system.restart.how.${current.supervisor ?? 'supervisor'}`)
  return t(`system.restart.how.${current.how}`)
})

const blocked = computed(() => {
  const current = status.value
  return current && !current.can_restart && current.blocked_reason ? translateServerMessage({ code: current.blocked_reason }) : null
})

const title = computed(() => {
  if (lost.value) return t('system.restart.lost_title')
  return going.value ? t('system.restart.restarting_title') : t('system.restart.title')
})
</script>

<template>
  <UAlert
    v-if="visible"
    :color="lost ? 'error' : 'warning'"
    :icon="going ? 'i-lucide-loader-circle' : 'i-lucide-rotate-ccw'"
    :title="title"
    :ui="going ? { icon: 'animate-spin' } : undefined"
    data-testid="restart-pending"
    aria-live="polite"
  >
    <template #description>
      <p v-if="lost" data-testid="restart-lost">{{ t('system.restart.lost') }}</p>
      <template v-else-if="going">
        <p>{{ t('system.restart.restarting_hint') }}</p>
        <!-- Nothing starts it again here: the person has to, and is told so while it goes down. -->
        <p v-if="status?.how === 'manual'" class="mt-1" data-testid="restart-how">{{ how }}</p>
      </template>
      <template v-else>
        <p v-if="reasons.length">{{ t('system.restart.intro') }}</p>
        <ul v-if="reasons.length" class="mt-1 list-disc ps-5" data-testid="restart-reasons">
          <li v-for="(line, index) in reasons" :key="index">{{ line }}</li>
        </ul>
        <p v-if="how" class="mt-2" data-testid="restart-how">{{ how }}</p>
        <p v-if="status?.automatic" class="mt-1 text-muted" data-testid="restart-automatic">{{ t('system.restart.automatic_hint') }}</p>
        <p v-if="blocked" class="mt-1" data-testid="restart-blocked">{{ blocked }}</p>
      </template>
    </template>
    <template v-if="!going && !lost" #actions>
      <UButton
        :label="t('system.restart.now')"
        icon="i-lucide-rotate-ccw"
        color="warning"
        size="sm"
        :loading="starting"
        :disabled="!status?.can_restart || starting"
        data-testid="restart-now"
        @click="restartNow()"
      />
    </template>
  </UAlert>
</template>
