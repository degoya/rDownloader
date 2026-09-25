<script setup lang="ts">
import type { NavigationMenuItem } from '@nuxt/ui'
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'

import CaptchaDialog from '@/components/CaptchaDialog.vue'
import LiveAnnouncer from '@/components/LiveAnnouncer.vue'
import NzbDropOverlay from '@/components/NzbDropOverlay.vue'
import PreferencesFooter from '@/components/PreferencesFooter.vue'
import TransferRail from '@/components/TransferRail.vue'
import { useAppShortcuts } from '@/composables/useAppShortcuts'
import { useAppTour } from '@/composables/useAppTour'
import { useFileImportDropZone } from '@/composables/useNzbDropZone'
import { sidebarCollapsed } from '@/composables/sidebarCollapse'
import { SETTINGS_SECTION_GROUPS } from '@/settingsSections'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { useSessionStore } from '@/stores/session'
import { useStreamsStore } from '@/stores/streams'
import { useTransfersStore } from '@/stores/transfers'

const transfers = useTransfersStore()
const collector = useCollectorStore()
const nzbImports = useNzbImportsStore()
const streams = useStreamsStore()
const session = useSessionStore()
const { startTour } = useAppTour()
const { t } = useI18n()
const route = useRoute()

// The wizard can only ask for the tour; it runs here, once the anchors it points at exist.
onMounted(async () => {
  if (!session.consumeTourRequest()) return
  await nextTick()
  startTour()
})
// App-wide NZB/torrent drag & drop (any route, while authenticated).
const { dropActive } = useFileImportDropZone()
// Global single-key shortcuts (any route, while authenticated); unbinds automatically on unmount.
useAppShortcuts()
// The listen port is configurable, so the address has to come from the connection the user
// actually reached the UI on rather than from a hardcoded default.
const endpoint = window.location.host
// Packages, not files — the file counts already live in the footer status bar.
const grabberCount = computed(() =>
  collector.packages.length
  + nzbImports.imports.filter(item => item.state === 'imported' || item.state === 'failed').length)
const downloadsBadge = computed(() =>
  transfers.packages.length ? `${transfers.activePackages}/${transfers.packages.length}` : undefined)
const settingsChildren = computed<NavigationMenuItem[]>(() => SETTINGS_SECTION_GROUPS.flatMap(group => [
  {
    label: t(group.labelKey),
    type: 'label' as const,
    value: `settings-group-${group.value}`,
    class: 'mt-2 first:mt-0'
  },
  ...group.sections.map(section => ({
    label: t(section.labelKey),
    icon: section.icon,
    to: `/settings/${section.value}`
  }))
]))
const inSettings = computed(() => route.path === '/settings' || route.path.startsWith('/settings/'))
/**
 * The open groups of the navigation, held here rather than left to `defaultOpen`: the menu reads
 * that once, when the shell mounts, and the shell mounts before the first route has resolved
 * and stays mounted across every later navigation. So the settings group was closed on every
 * settings page (RD-120-53). Entering the settings opens it; closing it by hand still works.
 */
const openGroups = ref<string[]>([])
watch(inSettings, (value) => {
  if (value && !openGroups.value.includes('settings')) openGroups.value = [...openGroups.value, 'settings']
}, { immediate: true })
function setOpenGroups(value: unknown): void {
  openGroups.value = Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []
}
const items = computed<NavigationMenuItem[][]>(() => [[
  { label: t('nav.downloads'), icon: 'i-lucide-arrow-down-to-line', to: '/downloads', ...(downloadsBadge.value ? { badge: downloadsBadge.value } : {}) },
  { label: t('nav.linkgrabber'), icon: 'i-lucide-magnet', to: '/linkgrabber', ...(grabberCount.value ? { badge: String(grabberCount.value) } : {}) },
  { label: t('nav.streams'), icon: 'i-lucide-radio', to: '/streams', ...(streams.channels.length ? { badge: String(streams.channels.length) } : {}) },
  { label: t('nav.subscriptions'), icon: 'i-lucide-rss', to: '/subscriptions' },
  // Beside the subscriptions, not under Accounts: a job that runs at a provider is something to
  // watch and answer, like a subscription's hits, and not a setting (RD-110-29).
  { label: t('nav.remote_jobs'), icon: 'i-lucide-cloud-cog', to: '/remote-jobs' },
  { label: t('nav.automation'), icon: 'i-lucide-workflow', to: '/automation' },
  { label: t('nav.stats'), icon: 'i-lucide-chart-column', to: '/stats' },
  { label: t('nav.logs'), icon: 'i-lucide-scroll-text', to: '/logs' },
  { label: t('nav.audit'), icon: 'i-lucide-shield-check', to: '/audit' }
], [
  {
    label: t('nav.settings'),
    icon: 'i-lucide-sliders-horizontal',
    to: '/settings',
    value: 'settings',
    slot: 'settings',
    // The pages are routes of their own, not children of `/settings`, so the router never calls
    // this entry active on them; the group is where the reader is, and says so.
    active: inSettings.value,
    children: settingsChildren.value
  }
]])
</script>

<template>
  <!-- First in the tab order on every page: the navigation is long, and reaching the content
       past it with a keyboard is otherwise thirty keystrokes. -->
  <a class="skip-link" href="#main-content">{{ t('common.a11y.skip_to_content') }}</a>
  <UDashboardGroup storage="localStorage" storage-key="rdownloader-dashboard">
    <!-- The sizes are shares of the window, and 15 % of a 1280 px window is 192 px: too narrow
         for "Téléchargements" beside its badge or for "rDownloader" beside the collapse
         switch. The open sidebar therefore never drops below 15rem (240 px); the collapsed
         rail keeps its 64 px, which is why the floor is tied to `data-collapsed` (RD-120-53).
         The ceiling is 21 % rather than 22 %: dragged to 22 % at 1280 px, the widened size
         column of the queue left a package name 192 px, under the 200 px `main.css` promises. -->
    <UDashboardSidebar
      v-model:collapsed="sidebarCollapsed"
      collapsible
      resizable
      :min-size="14"
      :max-size="21"
      :ui="{ root: 'data-[collapsed=false]:min-w-60' }"
    >
      <template #header="{ collapsed }">
        <!-- The collapsed rail is 64px wide (`min-w-16`) and its header keeps 32px between
             the paddings (`px-4`): room for exactly one control, and the 32px logo would fill
             it alone. Stacking the two would leave 56 of the header's 64px with no gap between
             them. So on the rail the logo *is* the switch (RD-110-32): the mark as the button's
             face, the panel icon hidden, the tooltip and the accessible name saying what a
             click does — the way back out of the rail has to stay on screen, and stated. -->
        <div v-if="!collapsed" class="flex min-w-0 items-center gap-3 px-1">
          <img src="/favicon.svg" alt="rDownloader" class="size-8 shrink-0" />
          <!-- No tagline under the name any more (owner's decision, RD-120-53): at 117 px of
               room it was cut off in all four languages, and it needed up to 223 px. -->
          <p class="min-w-0 truncate text-sm font-semibold text-highlighted">rDownloader</p>
        </div>
        <!-- Nuxt UI's own switch for this sidebar: it carries the panel icons and drives the
             dashboard context, so the state it changes is the one bound above. Only the name
             is ours, because the component's built-in one is English-only. One instance in
             both states rather than a `v-if` pair, so a keyboard user who presses it keeps
             their focus on it instead of losing it to the page. -->
        <UDashboardSidebarCollapse
          :class="collapsed
            ? 'mx-auto size-8 shrink-0 bg-contain bg-center bg-no-repeat p-0 [&>[data-slot=leadingIcon]]:hidden'
            : 'ml-auto shrink-0'"
          :style="collapsed ? { backgroundImage: 'url(/favicon.svg)' } : undefined"
          :aria-label="collapsed ? t('nav.expand_sidebar') : t('nav.collapse_sidebar')"
          :title="collapsed ? t('nav.expand_sidebar') : t('nav.collapse_sidebar')"
        />
      </template>

      <template #default="{ collapsed }">
        <nav data-tour="nav" :aria-label="t('common.a11y.main_navigation')">
          <!-- Labels wrap instead of being cut: with the settings group open, "Fonctionnement sans
               surveillance" needs 219 px of the 135 px a child entry has (RD-120-53). -->
          <UNavigationMenu
            :model-value="openGroups"
            :items="items"
            :collapsed="collapsed"
            :ui="{ linkLabel: 'whitespace-normal text-pretty' }"
            orientation="vertical"
            tooltip
            popover
            @update:model-value="setOpenGroups"
          >
            <!-- A collapsed NavigationMenu flattens its popover children into links. Render the
                 semantic headings explicitly so they do not turn into focusable fake links. -->
            <template #settings-content="{ close }">
              <div class="w-64 p-1">
                <section
                  v-for="(group, groupIndex) in SETTINGS_SECTION_GROUPS"
                  :key="group.value"
                  :class="groupIndex > 0 ? 'mt-2 border-t border-muted pt-2' : undefined"
                >
                  <p class="px-2 py-1 text-xs font-semibold text-toned">
                    {{ t(group.labelKey) }}
                  </p>
                  <ul>
                    <li v-for="section in group.sections" :key="section.value">
                      <ULink
                        v-slot="{ active }"
                        :to="`/settings/${section.value}`"
                        raw
                        exact
                        class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm font-medium focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                        active-class="bg-elevated text-highlighted"
                        inactive-class="text-default hover:bg-elevated/50 hover:text-highlighted"
                        @click="close?.()"
                      >
                        <UIcon
                          :name="section.icon"
                          class="size-5 shrink-0"
                          :class="active ? 'text-default' : 'text-dimmed'"
                        />
                        <span class="min-w-0 text-pretty">{{ t(section.labelKey) }}</span>
                      </ULink>
                    </li>
                  </ul>
                </section>
              </div>
            </template>
          </UNavigationMenu>
        </nav>
      </template>

      <template #footer="{ collapsed }">
        <!-- The footer slot is a flex row, so without `w-full` this block was as wide as its
             content: the selects' 165 px while the open sidebar is 240 px and more, and the
             56 px of dot, gap and sign-out on the 64 px rail, which pushed the separator 9 px
             past it. Full width, it runs where the navigation's separator runs, in both
             states; on the rail the dot and the sign-out stack, since side by side they need
             56 of the 32 px between the paddings (RD-120-61). -->
        <div data-testid="sidebar-footer" class="w-full min-w-0 border-t border-muted pt-2">
          <PreferencesFooter :collapsed="collapsed" />
          <UTooltip :text="t('nav.connected')" :disabled="!collapsed">
            <div
              data-testid="sidebar-connection"
              :class="collapsed ? 'flex flex-col items-center gap-2 py-1.5' : 'flex items-center gap-2 px-2 py-1.5'"
            >
              <!-- The pulse is decoration; what it means is in the label beside it, which is
                   what a screen reader reads. -->
              <span class="relative flex size-2 shrink-0" aria-hidden="true">
                <span class="absolute inline-flex size-full animate-ping bg-success opacity-50" />
                <span class="relative inline-flex size-2 bg-success" />
              </span>
              <span class="visually-hidden">{{ t('nav.connected') }}</span>
              <span v-if="!collapsed" class="font-mono text-xs text-toned">{{ endpoint }}</span>
              <!-- Beside the endpoint rather than in the navigation: signing out is not a
                   place to go, and the footer is where what applies to this session lives.
                   Hidden when sign-in is switched off, since there is no session to end. -->
              <UButton
                v-if="!session.loginDisabled"
                icon="i-lucide-log-out"
                size="xs"
                color="neutral"
                variant="ghost"
                :class="collapsed ? 'shrink-0' : 'ml-auto shrink-0'"
                :loading="session.pending"
                :aria-label="t('nav.sign_out')"
                :title="t('nav.sign_out')"
                @click="session.logout()"
              />
            </div>
          </UTooltip>
        </div>
      </template>
    </UDashboardSidebar>

    <!-- UDashboardPanel ships min-h-svh, which fills the viewport and pushes the
         TransferRail below this column's overflow-hidden; cap the panel instead. The variant
         has to sit on <main>, which is the panel's actual parent — on the wrapper below it is
         a child combinator against a grandchild, so it never matched and every route lost the
         bottom 44px (the rail's height) to the clip. -->
    <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
      <main id="main-content" tabindex="-1" class="flex min-h-0 flex-1 flex-col [&>[data-slot=root]]:min-h-0">
        <RouterView />
      </main>
      <TransferRail />
    </div>
  </UDashboardGroup>
  <LiveAnnouncer />
  <NzbDropOverlay v-if="dropActive" />
  <!-- Route-independent: a parked download can ask for a captcha from any view. -->
  <CaptchaDialog />
</template>
