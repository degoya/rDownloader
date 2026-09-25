<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'

import { api, responseError } from '@/api/client'
import type { ProxyProfile, Settings } from '@/api/types'
import SettingsAboutTab from '@/components/settings/SettingsAboutTab.vue'
import SettingsAccountsTab from '@/components/settings/SettingsAccountsTab.vue'
import SettingsBandwidthTab from '@/components/settings/SettingsBandwidthTab.vue'
import SettingsBackupRestore from '@/components/settings/SettingsBackupRestore.vue'
import SettingsCaptchaTab from '@/components/settings/SettingsCaptchaTab.vue'
import SettingsDesktopTab from '@/components/settings/SettingsDesktopTab.vue'
import SettingsGeneralTab from '@/components/settings/SettingsGeneralTab.vue'
import SettingsHotfoldersTab from '@/components/settings/SettingsHotfoldersTab.vue'
import SettingsInterfaceTab from '@/components/settings/SettingsInterfaceTab.vue'
import SettingsMcpTab from '@/components/settings/SettingsMcpTab.vue'
import SettingsMediaTab from '@/components/settings/SettingsMediaTab.vue'
import SettingsNetworkTab from '@/components/settings/SettingsNetworkTab.vue'
import SettingsNotificationsTab from '@/components/settings/SettingsNotificationsTab.vue'
import SettingsPluginsTab from '@/components/settings/SettingsPluginsTab.vue'
import SettingsPostprocessTab from '@/components/settings/SettingsPostprocessTab.vue'
import SettingsRoutingTab from '@/components/settings/SettingsRoutingTab.vue'
import SettingsSecurityTab from '@/components/settings/SettingsSecurityTab.vue'
import SettingsSiteRulesTab from '@/components/settings/SettingsSiteRulesTab.vue'
import SettingsServicesTab from '@/components/settings/SettingsServicesTab.vue'
import SettingsSystemTab from '@/components/settings/SettingsSystemTab.vue'
import SettingsToolsTab from '@/components/settings/SettingsToolsTab.vue'
import SettingsTorrentTab from '@/components/settings/SettingsTorrentTab.vue'
import SettingsTransfersTab from '@/components/settings/SettingsTransfersTab.vue'
import SettingsUnattendedTab from '@/components/settings/SettingsUnattendedTab.vue'
import SettingsUsenetTab from '@/components/settings/SettingsUsenetTab.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { defaultSettings } from '@/settingsDefaults'
import { SETTINGS_SECTIONS, settingsSection } from '@/settingsSections'
import { setByteDisplay, setByteUnit } from '@/utils/byteDisplay'
import { setShowItemImages } from '@/utils/itemImages'
import { setTitleStatus } from '@/utils/titleStatus'
import { MIB } from '@/utils/format'

const { t } = useI18n()
/** One shared settings object: the PUT replaces the whole document, so saving is global. */
const settings = reactive<Settings>(defaultSettings())
const proxies = ref<ProxyProfile[]>([])
const speedMiB = ref<number | null>(null)
const pending = ref(false)
const message = ref<string | null>(null)
const error = ref<string | null>(null)
const route = useRoute()
/**
 * The page comes from the URL (`/settings/network`), so it is deep-linkable, survives a reload
 * and can be reached from the sidebar. The router sends an unknown segment to the overview
 * before this view mounts, so the fallback here is never on screen.
 */
const activeSection = computed(() => settingsSection(route.params.section) ?? '')
/**
 * The navbar names the page, as every other view's does, rather than the whole area: the same
 * label as the sidebar entry that is highlighted beside it (RD-120-53).
 */
const pageTitle = computed(() => {
  const section = SETTINGS_SECTIONS.find(entry => entry.value === activeSection.value)
  return section ? t(section.labelKey) : t('settings.title')
})
const routingTab = ref('roots')
const confirm = useConfirm()
const systemTab = ref<InstanceType<typeof SettingsSystemTab> | null>(null)
const captchaTab = ref<InstanceType<typeof SettingsCaptchaTab> | null>(null)

/**
 * Pages bound to the settings document; the others (including backup/restore) save themselves.
 * Captcha is on the list although its document is a separate one: its card is saved by the same
 * button, through `saveCaptcha`.
 */
const DOCUMENT_TABS = [
  'general', 'interface', 'unattended', 'postprocess', 'captcha', 'torrent', 'media', 'transfers',
  'services', 'tools', 'network', 'security'
]
/** Routing saves itself everywhere except its collector pane, which edits the settings document. */
const showSaveBar = computed(() => DOCUMENT_TABS.includes(activeSection.value)
  || (activeSection.value === 'routing' && routingTab.value === 'collector'))

const { loading: proxiesLoading, loadError: proxiesError, load: trackProxies } = useFetchState()

onMounted(() => void Promise.all([load(), loadProxies()]))

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/settings')
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  applyLoadedSettings(response.data)
}

function applyLoadedSettings(value: Settings): void {
  Object.assign(settings, value)
  // Every view formats sizes through the same module, so applying it here makes a saved change
  // visible everywhere at once instead of after a reload.
  setByteDisplay(value.byte_display)
  setByteUnit(value.byte_unit)
  setShowItemImages(value.subscription_item_images_enabled)
  setTitleStatus(value.title_status_enabled)
  speedMiB.value = value.speed_limit_bytes_per_second
    ? Number(value.speed_limit_bytes_per_second) / MIB
    : null
}

/**
 * The proxy list the network tab renders. Its state travels with it: without one the tab
 * printed "no connection routes" for the duration of this request and for good if it failed
 * (RD-104-07).
 */
async function loadProxies(): Promise<void> {
  await trackProxies(async () => {
    const response = await api.GET('/api/v1/proxy-profiles')
    if (!response.data) return responseError(response)
    proxies.value = response.data
    return null
  })
}

async function handleSettingsImported(): Promise<void> {
  await Promise.all([load(), loadProxies(), systemTab.value?.refresh()])
}

async function save(): Promise<void> {
  pending.value = true
  message.value = null
  error.value = null
  settings.speed_limit_bytes_per_second = speedMiB.value
    ? String(Math.round(speedMiB.value * MIB))
    : null
  settings.global_proxy_profile_id = settings.global_proxy_profile_id || null
  settings.custom_ca_pem = settings.custom_ca_pem?.trim() || null
  settings.rar_executable = settings.rar_executable?.trim() || null
  settings.passwords_file = settings.passwords_file?.trim() || null
  settings.scripts_directory = settings.scripts_directory?.trim() || null
  settings.excluded_domains_file = settings.excluded_domains_file?.trim() || null
  settings.dlc_service_endpoint = settings.dlc_service_endpoint?.trim() || null
  settings.vendor_directory = settings.vendor_directory?.trim() || null
  settings.managed_tools_manifest_url = settings.managed_tools_manifest_url?.trim() || null
  settings.upload_remote = settings.upload_remote?.trim() || null
  settings.rclone_executable = settings.rclone_executable?.trim() || null
  settings.ui_port = settings.ui_port || null
  settings.cleanup_extensions = settings.cleanup_extensions
    .map(extension => extension.trim().replace(/^\./, '').toLowerCase())
    .filter((extension, index, all) => extension && all.indexOf(extension) === index)
  settings.media_ytdlp_executable = settings.media_ytdlp_executable?.trim() || null
  settings.media_ffmpeg_executable = settings.media_ffmpeg_executable?.trim() || null
  settings.gallery_executable = settings.gallery_executable?.trim() || null
  settings.record_streamlink_executable = settings.record_streamlink_executable?.trim() || null
  settings.record_default_quality = settings.record_default_quality.trim() || 'best'
  settings.gallery_hosts = settings.gallery_hosts
    .map(host => host.trim().toLowerCase().replace(/^https?:\/\//, '').replace(/^www\./, '').replace(/\/.*$/, ''))
    .filter((host, index, all) => host && all.indexOf(host) === index)
  settings.media_hosts = settings.media_hosts
    .map(host => host.trim().toLowerCase().replace(/^https?:\/\//, '').replace(/^www\./, '').replace(/\/.*$/, ''))
    .filter((host, index, all) => host && all.indexOf(host) === index)
  // Captcha credentials use a separate, redacted API document, but belong to the same UI save.
  const [response, captchaSaved] = await Promise.all([
    api.PUT('/api/v1/settings', { body: settings }),
    captchaTab.value?.saveCaptcha() ?? Promise.resolve(true)
  ])
  pending.value = false
  if (response.data) {
    // Not a bare Object.assign: the display preferences live in module refs that every view
    // formats through, and a save that only updated this component left the rest of the
    // interface on the old unit until the page was reloaded.
    applyLoadedSettings(response.data)
    if (captchaSaved) message.value = t('settings.messages.saved')
  } else {
    error.value = responseError(response)
  }
}

async function resetSettings(): Promise<void> {
  const confirmed = await confirm({
    title: t('settings.reset.title'),
    description: t('settings.reset.description'),
    confirmLabel: t('settings.reset.confirm'),
    confirmIcon: 'i-lucide-rotate-ccw',
    destructive: true
  })
  if (!confirmed) return
  pending.value = true
  message.value = null
  error.value = null
  const response = await api.POST('/api/v1/settings/reset')
  pending.value = false
  if (response.data) {
    applyLoadedSettings(response.data)
    message.value = t('settings.messages.reset_done')
  } else {
    error.value = responseError(response)
  }
}
</script>

<template>
  <UDashboardPanel id="settings">
    <template #header>
      <UDashboardNavbar :title="pageTitle">
        <template #leading><UDashboardSidebarCollapse /></template>
      </UDashboardNavbar>
    </template>
    <template #body>
      <!-- Not a <form>: the self-saving tabs (usenet, accounts, plugins) contain their own forms
           and nesting forms is invalid HTML. The save button calls save() directly instead. -->
      <div class="w-full space-y-6">
        <div class="w-full" data-tour="settings-tabs">
          <div v-if="activeSection === 'general'" class="pt-4">
            <SettingsGeneralTab :model-value="settings" v-model:speed-mib="speedMiB" />
          </div>
          <div v-if="activeSection === 'interface'" class="pt-4">
            <SettingsInterfaceTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'desktop'" class="pt-4">
            <SettingsDesktopTab />
          </div>
          <div v-if="activeSection === 'routing'" class="pt-4">
            <SettingsRoutingTab :model-value="settings" v-model:sub-tab="routingTab" />
          </div>
          <div v-if="activeSection === 'hotfolders'" class="pt-4">
            <SettingsHotfoldersTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'bandwidth'" class="pt-4">
            <SettingsBandwidthTab v-model="settings" />
          </div>
          <div v-if="activeSection === 'unattended'" class="pt-4">
            <SettingsUnattendedTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'postprocess'" class="pt-4">
            <SettingsPostprocessTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'accounts'" class="pt-4">
            <SettingsAccountsTab />
          </div>
          <div v-if="activeSection === 'captcha'" class="pt-4">
            <SettingsCaptchaTab ref="captchaTab" @error="(text: string) => (error = text)" />
          </div>
          <div v-if="activeSection === 'siterules'" class="pt-4">
            <SettingsSiteRulesTab />
          </div>
          <div v-if="activeSection === 'usenet'" class="pt-4">
            <SettingsUsenetTab />
          </div>
          <div v-if="activeSection === 'torrent'" class="pt-4">
            <SettingsTorrentTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'media'" class="pt-4">
            <SettingsMediaTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'transfers'" class="pt-4">
            <SettingsTransfersTab
              :model-value="settings"
              @message="(text: string) => (message = text)"
              @error="(text: string) => (error = text)"
            />
          </div>
          <div v-if="activeSection === 'services'" class="pt-4">
            <SettingsServicesTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'plugins'" class="pt-4">
            <SettingsPluginsTab />
          </div>
          <div v-if="activeSection === 'tools'" class="pt-4">
            <SettingsToolsTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'notifications'" class="pt-4">
            <SettingsNotificationsTab />
          </div>
          <div v-if="activeSection === 'mcp'" class="pt-4">
            <SettingsMcpTab />
          </div>
          <div v-if="activeSection === 'network'" class="pt-4">
            <SettingsNetworkTab
              :model-value="settings"
              v-model:proxies="proxies"
              :proxies-loading="proxiesLoading"
              :proxies-error="proxiesError"
              @message="(text: string) => (message = text)"
              @error="(text: string) => (error = text)"
            />
          </div>
          <div v-if="activeSection === 'security'" class="pt-4">
            <SettingsSecurityTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'backup'" class="pt-4">
            <SettingsBackupRestore @imported="handleSettingsImported" />
          </div>
          <div v-if="activeSection === 'about'" class="pt-4">
            <SettingsAboutTab />
          </div>
          <div v-if="activeSection === 'system'" class="pt-4">
            <SettingsSystemTab
              ref="systemTab"
              :model-value="settings"
              :resetting="pending"
              @reset="resetSettings"
            />
          </div>
        </div>

        <UAlert v-if="message" color="success" variant="subtle" icon="i-lucide-circle-check" :description="message" />
        <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
        <template v-if="showSaveBar">
          <div class="flex flex-wrap items-center justify-end gap-2">
            <UButton type="button" icon="i-lucide-rotate-ccw" :label="t('settings.reset.button')" color="neutral" variant="outline" :disabled="pending" @click="resetSettings" />
            <UButton type="button" icon="i-lucide-save" :label="t('settings.save')" :loading="pending" @click="save" />
          </div>
        </template>
      </div>
    </template>
  </UDashboardPanel>
</template>
