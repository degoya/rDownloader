<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { NetworkInterface, ProxyProfile, Settings, TorrentEngineCapabilities } from '@/api/types'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import { MIB, byteModel } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'
import { DECIMAL, PLAIN, RATIO, WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })

/** Interfaces the engine can bind to, plus the "all interfaces" default. */
const interfaces = ref<NetworkInterface[]>([])
const proxies = ref<ProxyProfile[]>([])
const proxyItems = computed(() => [
  { label: t('settings.torrent.proxy.none'), value: NO_SELECTION },
  ...proxies.value
    .filter(profile => profile.kind === 'socks5')
    .map(profile => ({ label: profile.name, value: profile.id }))
])
const proxyProfile = computed({
  get: () => optionalSelection(settings.value.torrent_proxy_profile_id),
  set: (value: string) => { settings.value.torrent_proxy_profile_id = selectionValue(value) }
})
const announcePort = computed({
  get: () => settings.value.torrent_announce_port ?? null,
  set: (value: number | null) => { settings.value.torrent_announce_port = value && value > 0 ? value : null }
})
const capabilities = ref<TorrentEngineCapabilities | null>(null)
const interfaceItems = computed(() => [
  { label: t('settings.torrent.bind_interface.any'), value: NO_SELECTION },
  ...interfaces.value
    .filter(entry => !entry.loopback)
    .map(entry => ({ label: `${entry.name} (${entry.addresses.join(', ')})`, value: entry.name }))
])
const bindInterface = computed({
  get: () => optionalSelection(settings.value.torrent_bind_interface),
  set: (value: string) => { settings.value.torrent_bind_interface = selectionValue(value) }
})
const listenModeItems = computed(() =>
  (['tcp_and_utp', 'tcp_only', 'utp_only'] as const).map(value => ({
    label: t(`settings.torrent.listen_mode.${value}`),
    value
  }))
)
const blocklistUrl = computed({
  get: () => settings.value.torrent_ip_blocklist_url ?? '',
  set: (value: string) => { settings.value.torrent_ip_blocklist_url = value.trim() || null }
})
const peerLimit = computed({
  get: () => settings.value.torrent_peer_limit ?? null,
  set: (value: number | null) => { settings.value.torrent_peer_limit = value && value > 0 ? value : null }
})

onMounted(async () => {
  const [found, matrix, profiles] = await Promise.all([
    api.GET('/api/v1/torrents/network/interfaces'),
    api.GET('/api/v1/torrents/capabilities'),
    api.GET('/api/v1/proxy-profiles')
  ])
  if (found.data) interfaces.value = found.data
  if (matrix.data) capabilities.value = matrix.data
  if (profiles.data) proxies.value = profiles.data
})
const { t } = useI18n()

/** Seeding is a form of sharing, so the sharing switch is the outer one of the two. */
const seeding = computed(() => settings.value.torrent_sharing_enabled && settings.value.torrent_seeding_enabled)

const listenPort = computed({
  get: () => settings.value.torrent_listen_port ?? undefined,
  set: (value: number | undefined) => {
    settings.value.torrent_listen_port = value && Number.isFinite(value) ? value : null
  }
})
const seedTime = computed({
  get: () => settings.value.torrent_seed_time_minutes ?? undefined,
  set: (value: number | undefined) => {
    settings.value.torrent_seed_time_minutes = value && Number.isFinite(value) ? value : null
  }
})
const uploadLimitMiB = byteModel(
  () => settings.value.torrent_upload_limit_bytes_per_second,
  (raw) => { settings.value.torrent_upload_limit_bytes_per_second = raw },
  MIB
)
</script>

<template>
  <UCard as="section" data-settings-anchor="torrent.settings" :ui="{ body: 'space-y-4' }">
    <div>
      <SectionHeader
        :eyebrow="t('settings.torrent.eyebrow')"
        :title="t('settings.torrent.title')"
        :description="t('settings.torrent.description')"
        level="sub"
      />
    </div>
    <UFormField :label="t('settings.torrent.sharing.label')" orientation="horizontal">
      <template #description>
        {{ t('settings.torrent.sharing.description') }}
        <span v-if="!settings.torrent_sharing_enabled" class="mt-1 block text-warning">
          {{ t('settings.torrent.sharing.leech_only_warning') }}
        </span>
      </template>
      <USwitch v-model="settings.torrent_sharing_enabled" data-testid="torrent-sharing" />
    </UFormField>
    <UFormField hint="MiB/s" data-settings-anchor="torrent.upload_limit" :label="t('settings.torrent.upload_limit.label')" :description="t('settings.torrent.upload_limit.description')">
      <UInputNumber v-model="uploadLimitMiB" :min="0" :format-options="DECIMAL" :step-snapping="false" :disabled="!settings.torrent_sharing_enabled" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.torrent.seeding.label')" :description="t('settings.torrent.seeding.description')" orientation="horizontal">
      <USwitch v-model="settings.torrent_seeding_enabled" :disabled="!settings.torrent_sharing_enabled" />
    </UFormField>
    <UFormField data-settings-anchor="torrent.seed_ratio" :label="t('settings.torrent.seed_ratio.label')" :description="t('settings.torrent.seed_ratio.description')">
      <UInputNumber v-model="settings.torrent_seed_ratio" required :min="0" :max="100" :format-options="RATIO" :step-snapping="false" :disabled="!seeding" class="w-full" />
    </UFormField>
    <UFormField hint="min" :label="t('settings.torrent.seed_time.label')" :description="t('settings.torrent.seed_time.description')">
      <UInputNumber v-model="seedTime" :min="1" :format-options="WHOLE" :disabled="!seeding" class="w-full" />
    </UFormField>
    <UFormField
      data-settings-anchor="torrent.bind_interface"
      v-if="capabilities?.interface_binding"
      :label="t('settings.torrent.bind_interface.label')"
      :description="t('settings.torrent.bind_interface.description')"
    >
      <USelect v-model="bindInterface" :items="interfaceItems" value-key="value" class="w-full" />
    </UFormField>
    <UFormField
      data-settings-anchor="torrent.kill_switch"
      v-if="capabilities?.interface_binding"
      :label="t('settings.torrent.kill_switch.label')"
      :description="t('settings.torrent.kill_switch.description')"
      orientation="horizontal"
    >
      <USwitch v-model="settings.torrent_kill_switch_enabled" :disabled="!settings.torrent_bind_interface" />
    </UFormField>
    <UFormField
      :label="t('settings.torrent.listen_mode.label')"
      :description="t('settings.torrent.listen_mode.description')"
    >
      <USelect v-model="settings.torrent_listen_mode" :items="listenModeItems" value-key="value" class="w-full" />
    </UFormField>
    <UFormField data-settings-anchor="torrent.listen_port" :label="t('settings.torrent.listen_port.label')" :description="t('settings.torrent.listen_port.description')">
      <UInputNumber v-model="listenPort" :min="1" :max="65535" :format-options="PLAIN" class="w-full" />
    </UFormField>
    <UFormField
      :label="t('settings.torrent.peer_limit.label')"
      :description="t('settings.torrent.peer_limit.description')"
    >
      <UInputNumber v-model="peerLimit" :min="0" :format-options="WHOLE" class="w-full" />
    </UFormField>
    <UFormField
      v-if="capabilities?.socks5_peer_proxy"
      :label="t('settings.torrent.proxy.label')"
      :description="t('settings.torrent.proxy.description')"
    >
      <USelect v-model="proxyProfile" :items="proxyItems" value-key="value" class="w-full" />
    </UFormField>
    <UFormField
      data-settings-anchor="torrent.upnp"
      v-if="capabilities?.upnp"
      :label="t('settings.torrent.upnp.label')"
      :description="t('settings.torrent.upnp.description')"
      orientation="horizontal"
    >
      <USwitch v-model="settings.torrent_upnp_enabled" />
    </UFormField>
    <UFormField
      :label="t('settings.torrent.announce_port.label')"
      :description="t('settings.torrent.announce_port.description')"
    >
      <UInputNumber v-model="announcePort" :min="0" :max="65535" :format-options="PLAIN" class="w-full" />
    </UFormField>
    <UFormField
      data-settings-anchor="torrent.blocklist"
      v-if="capabilities?.ip_blocklist_url"
      :label="t('settings.torrent.blocklist.label')"
      :description="t('settings.torrent.blocklist.description')"
    >
      <UInput v-model="blocklistUrl" type="url" placeholder="https://example.com/blocklist.txt" class="w-full" />
    </UFormField>
    <UFormField
      :label="t('settings.torrent.peer_addresses.label')"
      :description="t('settings.torrent.peer_addresses.description')"
      orientation="horizontal"
    >
      <USwitch v-model="settings.torrent_peer_addresses_visible" />
    </UFormField>
    <p class="text-xs leading-5 text-muted">{{ t('settings.torrent.restart_hint') }}</p>
  </UCard>
</template>
