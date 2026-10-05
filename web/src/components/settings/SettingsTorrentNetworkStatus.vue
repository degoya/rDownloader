<script setup lang="ts">
/**
 * What the torrent engine's network layer is doing right now (RD-190-22).
 *
 * `GET /api/v1/torrents/network/status` existed since the kill switch did, and nothing showed
 * it: a VPN user never learnt that the kill switch had paused every torrent, or that the session
 * rebuild after a saved change had failed and the old session was still running. The card reads
 * it when the page opens and on demand; the downloads view has its own warning while the kill
 * switch holds the torrents (`TorrentKillSwitchAlert`).
 */
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { TorrentNetworkStatus } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const { t } = useI18n()
const status = ref<TorrentNetworkStatus | null>(null)
const error = ref<string | null>(null)
const loading = ref(false)

async function load(): Promise<void> {
  loading.value = true
  const response = await api.GET('/api/v1/torrents/network/status')
  loading.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  error.value = null
  status.value = response.data
}

onMounted(() => void load())

defineExpose({ reload: load })
</script>

<template>
  <UCard as="section" data-settings-anchor="torrent.network_status" :ui="{ body: 'space-y-4' }" data-testid="torrent-network-status">
    <div class="flex items-start justify-between gap-4">
      <SectionHeader
        :eyebrow="t('settings.torrent.eyebrow')"
        :title="t('settings.torrent.network_status.title')"
        :description="t('settings.torrent.network_status.description')"
        level="sub"
      />
      <UButton
        size="xs"
        color="neutral"
        variant="ghost"
        icon="i-lucide-refresh-cw"
        :aria-label="t('common.actions.refresh')"
        :title="t('common.actions.refresh')"
        :loading="loading"
        @click="load"
      />
    </div>
    <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
    <template v-if="status">
      <UAlert
        v-if="status.kill_switch_engaged"
        color="warning"
        variant="subtle"
        icon="i-lucide-shield-alert"
        :title="t('settings.torrent.network_status.engaged_title')"
        :description="t('settings.torrent.network_status.engaged_description', { name: status.bound_interface ?? '' })"
      />
      <UAlert
        v-if="status.last_rebuild_error"
        color="error"
        variant="subtle"
        icon="i-lucide-circle-alert"
        :title="t('settings.torrent.network_status.rebuild_failed')"
        :description="status.last_rebuild_error"
      />
      <UAlert
        v-if="status.peer_proxy_error"
        color="error"
        variant="subtle"
        icon="i-lucide-circle-alert"
        :title="t('settings.torrent.network_status.proxy_failed')"
        :description="status.peer_proxy_error"
      />
      <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]">
        <dt class="text-muted">{{ t('settings.torrent.network_status.interface') }}</dt>
        <dd class="flex flex-wrap items-center gap-2">
          <template v-if="status.bound_interface">
            <span class="font-mono text-highlighted">{{ status.bound_interface }}</span>
            <UBadge :color="status.bound_interface_present ? 'success' : 'warning'" variant="subtle">
              {{ status.bound_interface_present ? t('settings.torrent.network_status.interface_present') : t('settings.torrent.network_status.interface_missing') }}
            </UBadge>
          </template>
          <span v-else class="text-highlighted">{{ t('settings.torrent.bind_interface.any') }}</span>
        </dd>
        <dt class="text-muted">{{ t('settings.torrent.network_status.kill_switch') }}</dt>
        <dd data-testid="kill-switch-state" :class="status.kill_switch_engaged ? 'font-medium text-warning' : 'text-highlighted'">
          <template v-if="status.kill_switch_engaged">{{ t('settings.torrent.network_status.kill_switch_engaged') }}</template>
          <template v-else-if="status.kill_switch_enabled && status.bound_interface">
            {{ t('settings.torrent.network_status.kill_switch_armed', { seconds: status.kill_switch_window_seconds }) }}
          </template>
          <template v-else>{{ t('settings.torrent.network_status.kill_switch_off') }}</template>
        </dd>
        <dt class="text-muted">{{ t('settings.torrent.network_status.peer_proxy') }}</dt>
        <dd class="text-highlighted">
          {{ status.peer_proxy_configured ? t('settings.torrent.network_status.peer_proxy_on') : t('settings.torrent.network_status.peer_proxy_off') }}
        </dd>
      </dl>
    </template>
  </UCard>
</template>
