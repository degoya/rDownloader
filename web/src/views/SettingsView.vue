<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'

import { api, responseError } from '@/api/client'
import type { Settings } from '@/api/types'
import DataState from '@/components/DataState.vue'
import { clearWhenReconnected, onServiceReconnected } from '@/composables/serviceConnection'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { useLazySection } from '@/composables/useLazySection'
import { provideSettingsDocument } from '@/composables/useSettingsDocument'
import { useSettingsDraft } from '@/composables/useSettingsDraft'
import { useSettingsSubTab } from '@/composables/useSettingsSubTab'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import { emptyNumberFields } from '@/settingsDefaults'
import { SETTINGS_SECTIONS, settingsSection } from '@/settingsSections'
import { useProxyProfiles } from '@/stores/proxyProfiles'
import { useSettingsStore } from '@/stores/settings'
import { useTransfersStore } from '@/stores/transfers'
import { setByteDisplay, setByteUnit } from '@/utils/byteDisplay'
import { setShowItemImages } from '@/utils/itemImages'
import { setShowNzbHandOver } from '@/utils/nzbHandOver'
import { setShowPackageHandOver } from '@/utils/packageHandOver'
import { setPackagesClosedByDefault } from '@/utils/packageGroups'
import { setTitleStatus } from '@/utils/titleStatus'

const { t } = useI18n()
/** One chunk per page: the URL shows one of them, and together they were 654 KB (RD-1480-18). */
const lazySection = useLazySection()
const SettingsAboutTab = lazySection(() => import('@/components/settings/SettingsAboutTab.vue'))
const SettingsAccountsTab = lazySection(() => import('@/components/settings/SettingsAccountsTab.vue'))
const SettingsBandwidthTab = lazySection(() => import('@/components/settings/SettingsBandwidthTab.vue'))
const SettingsBackupRestore = lazySection(() => import('@/components/settings/SettingsBackupRestore.vue'))
const SettingsCaptchaTab = lazySection(() => import('@/components/settings/SettingsCaptchaTab.vue'))
const SettingsClientsTab = lazySection(() => import('@/components/settings/SettingsClientsTab.vue'))
const SettingsGeneralTab = lazySection(() => import('@/components/settings/SettingsGeneralTab.vue'))
const SettingsHotfoldersTab = lazySection(() => import('@/components/settings/SettingsHotfoldersTab.vue'))
const SettingsInterfaceTab = lazySection(() => import('@/components/settings/SettingsInterfaceTab.vue'))
const SettingsLinkgrabberTab = lazySection(() => import('@/components/settings/SettingsLinkgrabberTab.vue'))
const SettingsMediaTab = lazySection(() => import('@/components/settings/SettingsMediaTab.vue'))
const SettingsNetworkTab = lazySection(() => import('@/components/settings/SettingsNetworkTab.vue'))
const SettingsNotificationsTab = lazySection(() => import('@/components/settings/SettingsNotificationsTab.vue'))
const SettingsPluginsTab = lazySection(() => import('@/components/settings/SettingsPluginsTab.vue'))
const SettingsPostprocessTab = lazySection(() => import('@/components/settings/SettingsPostprocessTab.vue'))
const SettingsRoutingTab = lazySection(() => import('@/components/settings/SettingsRoutingTab.vue'))
const SettingsSecurityTab = lazySection(() => import('@/components/settings/SettingsSecurityTab.vue'))
const SettingsSiteRulesTab = lazySection(() => import('@/components/settings/SettingsSiteRulesTab.vue'))
const SettingsServicesTab = lazySection(() => import('@/components/settings/SettingsServicesTab.vue'))
const SettingsSystemTab = lazySection(() => import('@/components/settings/SettingsSystemTab.vue'))
const SettingsToolsTab = lazySection(() => import('@/components/settings/SettingsToolsTab.vue'))
const SettingsTorrentTab = lazySection(() => import('@/components/settings/SettingsTorrentTab.vue'))
const SettingsTransfersTab = lazySection(() => import('@/components/settings/SettingsTransfersTab.vue'))
const SettingsUnattendedTab = lazySection(() => import('@/components/settings/SettingsUnattendedTab.vue'))
const SettingsUsenetTab = lazySection(() => import('@/components/settings/SettingsUsenetTab.vue'))
const transfers = useTransfersStore()
const { proxies, fetchProxies } = useProxyProfiles()
const { fetchSettings } = useSettingsStore()
const pending = ref(false)
/**
 * Whether the document on screen is the stored one. Until it is, `settings` holds the
 * placeholders of `defaultSettings()`, and an edit of one would be saved as a choice (WEB-01).
 * The pages bound to the document, the save bar and the reset wait for it.
 */
const loaded = ref(false)
/** One shared settings object over every page, so saving is global: it sends what changed (RD-1480-07). */
const { settings, speedMiB, foreign, dirty, replace, takeForeign, save: saveDraft } = useSettingsDraft({
  loaded: () => loaded.value,
  busy: () => pending.value,
  onStored: applyLoadedSettings
})
/** Why the document could not be loaded, shown in place of the pages bound to it. */
const loadError = ref<string | null>(null)
const message = ref<string | null>(null)
const error = ref<string | null>(null)
clearWhenReconnected(error)
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
/**
 * The sub-tab of a page that has them, from `?tab=` (RD-180-15). Owned here rather than by each
 * page because the save bar below depends on it, and one page is mounted at a time.
 */
const { tabs: subTabs, active: subTab } = useSettingsSubTab(() => activeSection.value)
const confirm = useConfirm()
const systemTab = ref<InstanceType<typeof SettingsSystemTab> | null>(null)
const captchaTab = ref<InstanceType<typeof SettingsCaptchaTab> | null>(null)
const networkTab = ref<InstanceType<typeof SettingsNetworkTab> | null>(null)

/**
 * Pages bound to the settings document; the others (including backup/restore) save themselves.
 * Captcha is on the list although its document is a separate one: its card is saved by the same
 * button, through `saveCaptcha`.
 */
const DOCUMENT_TABS = [
  'general', 'unattended', 'captcha', 'torrent', 'services'
]
/**
 * On a page with sub-tabs the tab decides: routing saves itself on its categories and rules,
 * and network, security, bandwidth, Usenet, system, FTP, SFTP, WebDAV & S3, LinkGrabber,
 * Interface, post-processing and tools only have document fields on some of theirs (`saveBar` or `documentCard` in `SETTINGS_SUB_TABS`). System had none until RD-180-15: its
 * update and retention fields were saved only by the button of another page.
 */
const showSaveBar = computed(() => loaded.value && (subTabs.value.length
  ? subTabs.value.some(tab => tab.value === subTab.value && (tab.saveBar || tab.documentCard))
  : DOCUMENT_TABS.includes(activeSection.value)))

/**
 * Pages without sub-tabs that render fields of the document, the save bar's pages among them.
 * Until the document is loaded they show its state instead of a form of placeholders; the
 * self-saving pages do not wait for it.
 */
const BOUND_TABS = new Set([...DOCUMENT_TABS, 'hotfolders'])
/**
 * On a page with sub-tabs the sub-tab decides, not the page (RA-WEB-05): categories, rules,
 * sign-in or the authentication profiles save themselves and stay usable when the document
 * cannot be loaded; only a tab with its fields (`saveBar`) or its values (`showsDocument`) waits.
 * A tab with one card of the document among its own (`documentCard`) does not: the card waits.
 */
const waitingForDocument = computed(() => {
  if (loaded.value) return false
  if (!subTabs.value.length) return BOUND_TABS.has(activeSection.value)
  const tab = subTabs.value.find(entry => entry.value === subTab.value)
  return Boolean(tab?.saveBar || tab?.showsDocument)
})
/** The sub-tabs above the document's state, so a waiting tab is not a dead end on its page. */
const waitingTabItems = computed(() => subTabs.value.map(tab => ({ value: tab.value, label: t(tab.labelKey), icon: tab.icon })))

/** An obligatory number field the person emptied holds the save until it has a value again. */
const numberEmpty = computed(() => emptyNumberFields(settings).length > 0)
const captchaDirty = computed(() => captchaTab.value?.dirty ?? false)
const proxyDirty = computed(() => networkTab.value?.proxyDirty ?? false)
/**
 * Another page or sub-tab of this view keeps the document, which lives here; only the captcha
 * card's own form and the proxy form unmount with their page. So a switch inside the settings
 * asks only for those, and leaving the settings or closing the tab asks for any of them.
 */
useUnsavedGuard(
  () => dirty() || captchaDirty.value || proxyDirty.value,
  {
    dropsEdits: to => (captchaDirty.value && to.params.section !== 'captcha')
      || (proxyDirty.value && to.params.section !== 'network')
  }
)

const { loading: proxiesLoading, loadError: proxiesError, load: trackProxies } = useFetchState()

provideSettingsDocument({ loaded, loadError, retry: load })

onMounted(() => void Promise.all([load(), loadProxies()]))
// A document that never arrived is fetched again once the service is back.
onUnmounted(onServiceReconnected(() => {
  if (!loaded.value) void load()
}))

async function load(): Promise<void> {
  if (!loaded.value) loadError.value = null
  const response = await fetchSettings()
  if (!response.data) {
    // Before the first successful load the failure stands in place of the pages; afterwards the
    // form holds the stored document and the failure is one more message beside it.
    if (loaded.value) error.value = responseError(response)
    else loadError.value = responseError(response)
    return
  }
  replace(response)
  loaded.value = true
  loadError.value = null
}

function applyLoadedSettings(value: Settings): void {
  // Every view formats sizes through the same module, so applying it here makes a saved change
  // visible everywhere at once instead of after a reload.
  setByteDisplay(value.byte_display)
  setByteUnit(value.byte_unit)
  setShowItemImages(value.subscription_item_images_enabled)
  setShowNzbHandOver(value)
  setShowPackageHandOver(value)
  setTitleStatus(value.title_status_enabled)
  setPackagesClosedByDefault(value)
  // The status bar sets the speed limit and the parallel downloads too (RD-1120-22); what this
  // page loaded or saved is what the bar shows from now on.
  transfers.applyRailSettings(value)
}

/**
 * The proxy list the network tab renders. Its state travels with it: without one the tab
 * printed "no connection routes" for the duration of this request and for good if it failed
 * (RD-104-07).
 */
async function loadProxies(): Promise<void> {
  await trackProxies(async () => {
    const response = await fetchProxies()
    return response.data ? null : responseError(response)
  })
}

async function handleSettingsImported(): Promise<void> {
  await Promise.all([load(), loadProxies(), systemTab.value?.refresh()])
}

async function save(): Promise<void> {
  if (!loaded.value || numberEmpty.value) return
  pending.value = true
  message.value = null
  error.value = null
  // Captcha credentials use a separate, redacted API document, but belong to the same UI save.
  const [outcome, captchaSaved] = await Promise.all([
    saveDraft(),
    captchaTab.value?.saveCaptcha() ?? Promise.resolve(true)
  ])
  pending.value = false
  // A conflict is the notice above the save bar, which names the fields.
  if ('refused' in outcome) error.value = responseError(outcome.refused)
  else if ('saved' in outcome && captchaSaved) message.value = t('settings.messages.saved')
}

async function resetSettings(): Promise<void> {
  if (!loaded.value) return
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
    replace(response)
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
        <div v-if="waitingForDocument" class="space-y-3 pt-4" data-testid="settings-document-state">
          <UTabs v-if="waitingTabItems.length" v-model="subTab" :items="waitingTabItems" :content="false" variant="pill" class="w-full" />
          <DataState :loading="!loadError" :error="loadError" :rows="6" />
          <div v-if="loadError" class="flex justify-end">
            <UButton type="button" icon="i-lucide-refresh-cw" :label="t('common.actions.retry')" color="neutral" variant="outline" @click="load" />
          </div>
        </div>
        <div v-else class="w-full" data-tour="settings-tabs">
          <div v-if="activeSection === 'general'" class="pt-4">
            <SettingsGeneralTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'interface'" class="pt-4">
            <SettingsInterfaceTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'routing'" class="pt-4">
            <SettingsRoutingTab :model-value="settings" v-model:sub-tab="subTab" />
          </div>
          <div v-if="activeSection === 'hotfolders'" class="pt-4">
            <SettingsHotfoldersTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'linkgrabber'" class="pt-4">
            <SettingsLinkgrabberTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'bandwidth'" class="pt-4">
            <SettingsBandwidthTab v-model="settings" v-model:speed-mib="speedMiB" v-model:sub-tab="subTab" />
          </div>
          <div v-if="activeSection === 'unattended'" class="pt-4">
            <SettingsUnattendedTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'postprocess'" class="pt-4">
            <SettingsPostprocessTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'accounts'" class="pt-4">
            <SettingsAccountsTab v-model:sub-tab="subTab" />
          </div>
          <div v-if="activeSection === 'captcha'" class="pt-4">
            <SettingsCaptchaTab ref="captchaTab" @error="(text: string) => (error = text)" />
          </div>
          <div v-if="activeSection === 'siterules'" class="pt-4">
            <SettingsSiteRulesTab />
          </div>
          <div v-if="activeSection === 'usenet'" class="pt-4">
            <SettingsUsenetTab v-model:sub-tab="subTab" :model-value="settings" :service-off="!settings.usenet_service_enabled" />
          </div>
          <div v-if="activeSection === 'torrent'" class="pt-4">
            <SettingsTorrentTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'media'" class="pt-4">
            <SettingsMediaTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'transfers'" class="pt-4">
            <SettingsTransfersTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'services'" class="pt-4">
            <SettingsServicesTab :model-value="settings" />
          </div>
          <div v-if="activeSection === 'plugins'" class="pt-4">
            <SettingsPluginsTab v-model:sub-tab="subTab" />
          </div>
          <div v-if="activeSection === 'tools'" class="pt-4">
            <SettingsToolsTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'notifications'" class="pt-4">
            <SettingsNotificationsTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'clients'" class="pt-4">
            <SettingsClientsTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'network'" class="pt-4">
            <SettingsNetworkTab
              ref="networkTab"
              :model-value="settings"
              v-model:proxies="proxies"
              v-model:sub-tab="subTab"
              :proxies-loading="proxiesLoading"
              :proxies-error="proxiesError"
            />
          </div>
          <div v-if="activeSection === 'security'" class="pt-4">
            <SettingsSecurityTab v-model:sub-tab="subTab" :model-value="settings" />
          </div>
          <div v-if="activeSection === 'backup'" class="pt-4">
            <SettingsBackupRestore v-model:sub-tab="subTab" @imported="handleSettingsImported" />
          </div>
          <div v-if="activeSection === 'about'" class="pt-4">
            <SettingsAboutTab v-model:sub-tab="subTab" />
          </div>
          <div v-if="activeSection === 'system'" class="pt-4">
            <SettingsSystemTab
              ref="systemTab"
              v-model:sub-tab="subTab"
              :model-value="settings"
              :resetting="pending"
              @reset="resetSettings"
            />
          </div>
        </div>

        <UAlert
          v-if="foreign"
          :color="foreign.conflicts.length ? 'warning' : 'info'"
          role="status"
          icon="i-lucide-git-merge"
          :title="t('settings.foreign.title')"
          :description="foreign.conflicts.length ? t('settings.foreign.conflict', { fields: foreign.conflicts.join(', ') }) : t('settings.foreign.merged')"
          :close="!foreign.conflicts.length"
          data-testid="settings-foreign"
          @update:open="foreign = null"
        >
          <template v-if="foreign.conflicts.length" #actions>
            <UButton type="button" icon="i-lucide-undo-2" :label="t('settings.foreign.take')" color="neutral" variant="outline" size="sm" @click="takeForeign" />
          </template>
        </UAlert>
        <UAlert v-if="message" color="success" icon="i-lucide-circle-check" :description="message" />
        <UAlert v-if="error" color="error" role="alert" icon="i-lucide-circle-alert" :description="error" />
        <template v-if="showSaveBar">
          <div class="flex flex-wrap items-center justify-end gap-2">
            <p v-if="numberEmpty" class="text-sm text-error" data-testid="settings-number-empty">{{ t('settings.messages.number_empty') }}</p>
            <!-- System carries the same reset in its own header, on every tab. -->
            <UButton v-if="activeSection !== 'system'" type="button" icon="i-lucide-rotate-ccw" :label="t('settings.reset.button')" color="neutral" variant="outline" :disabled="pending" @click="resetSettings" />
            <UButton type="button" icon="i-lucide-save" :label="t('settings.save')" :loading="pending" :disabled="numberEmpty" @click="save" />
          </div>
        </template>
      </div>
    </template>
  </UDashboardPanel>
</template>
