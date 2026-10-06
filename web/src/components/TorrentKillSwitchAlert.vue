<script setup lang="ts">
/**
 * The downloads view's warning while the torrent kill switch holds every torrent (RD-190-22).
 *
 * The kill switch pauses all torrents when the bound interface — a VPN tunnel, as a rule —
 * disappears, and resumes them when it returns. Until now nothing said so: the torrents simply
 * stood still. The warning stands with the view's other state alerts (power countdown, full
 * storage), reads the status as often as the kill switch checks the interface, and goes away on
 * its own once the torrents run again.
 */
import { onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { api } from '@/api/client'
import type { TorrentNetworkStatus } from '@/api/types'
import { revealAnchor } from '@/utils/revealAnchor'

/** `INTERFACE_CHECK_INTERVAL` in `crates/rd-torrent/src/network.rs` is 5 s; the window twice that. */
const POLL_MS = 10_000

const { t } = useI18n()
const router = useRouter()
const status = ref<TorrentNetworkStatus | null>(null)
let timer: ReturnType<typeof setInterval> | null = null

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/torrents/network/status')
  // A failed read keeps the last answer: the warning is a hint, not the only way to find out.
  if (response.data) status.value = response.data
}

async function openStatus(): Promise<void> {
  await router.push('/settings/torrent')
  void revealAnchor('torrent.network_status', { focus: false })
}

onMounted(() => {
  void load()
  timer = setInterval(() => void load(), POLL_MS)
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
})
</script>

<template>
  <UAlert
    v-if="status?.kill_switch_engaged"
    color="warning"
    icon="i-lucide-shield-alert"
    :title="t('downloads.kill_switch.title')"
    :description="t('downloads.kill_switch.description', { name: status.bound_interface ?? '' })"
    data-testid="kill-switch-alert"
  >
    <template #actions>
      <UButton
        size="xs"
        color="warning"
        variant="outline"
        icon="i-lucide-network"
        :label="t('downloads.kill_switch.open')"
        @click="openStatus"
      />
    </template>
  </UAlert>
</template>
