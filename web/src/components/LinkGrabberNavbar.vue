<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import NavbarAction from '@/components/NavbarAction.vue'
import { shortcutKeys } from '@/composables/shortcutDefinitions'

/** The LinkGrabber's navbar: its actions as icons, each key in its tooltip, and on a phone one menu (1.8.1, RD-1260-06). */
const props = defineProps<{
  importing: boolean
  checking: boolean
  /** Something the online check can take: the list shows links. */
  canCheck: boolean
  hasEntries: boolean
  enqueuing: boolean
}>()
const emit = defineEmits<{
  add: []
  import: []
  history: []
  /** Open the indexer search drawer (RD-1230-02). */
  search: []
  check: []
  enqueue: [paused: boolean]
  clearAll: []
}>()
const { t } = useI18n()

/*
 * The navbar measures its own width (`@container`), not the window's. Every action is an icon
 * with its name and key in the tooltip, as in the Downloads navbar (RD-1260-06); on a phone what
 * is neither adding nor enqueuing moves into one menu, its keys shown there.
 */
const NAV_WIDE = 'hidden @min-[40rem]:inline-flex'
const navbarMenu = computed(() => [[
  { label: t('linkgrabber.actions.import_files'), icon: 'i-lucide-file-up', kbds: shortcutKeys('common.shortcuts.import_nzb'), onSelect: () => emit('import') },
  { label: t('linkgrabber.nzb.history.title'), icon: 'i-lucide-history', onSelect: () => emit('history') },
  { label: t('linkgrabber.search.open'), icon: 'i-lucide-search', kbds: shortcutKeys('common.shortcuts.focus_indexer_search'), onSelect: () => emit('search') },
  { label: t('linkgrabber.actions.check_links'), icon: 'i-lucide-radar', disabled: !props.canCheck, onSelect: () => emit('check') },
  { label: t('linkgrabber.actions.enqueue_paused'), icon: 'i-lucide-pause', kbds: shortcutKeys('common.shortcuts.enqueue_paused'), disabled: !props.hasEntries || props.checking, onSelect: () => emit('enqueue', true) }
], [
  { label: t('linkgrabber.actions.clear_all'), icon: 'i-lucide-list-x', color: 'error' as const, kbds: shortcutKeys('common.shortcuts.clear_linkgrabber'), disabled: !props.hasEntries, onSelect: () => emit('clearAll') }
]])
</script>

<template>
  <UDashboardNavbar :title="t('linkgrabber.title')" :ui="{ root: '@container' }">
    <template #leading><UDashboardSidebarCollapse /></template>
    <template #right>
      <div data-tour="grabber-add" class="flex items-center gap-2">
        <NavbarAction icon="i-lucide-plus" :label="t('linkgrabber.actions.add_links')" shortcut="common.shortcuts.add_links" color="neutral" variant="outline" @click="emit('add')" />
        <NavbarAction icon="i-lucide-file-up" :label="t('linkgrabber.actions.import_files')" shortcut="common.shortcuts.import_nzb" :class="NAV_WIDE" color="neutral" variant="outline" :loading="props.importing" @click="emit('import')" />
        <NavbarAction icon="i-lucide-history" :label="t('linkgrabber.nzb.history.title')" :class="NAV_WIDE" color="neutral" variant="outline" @click="emit('history')" />
        <NavbarAction icon="i-lucide-search" :label="t('linkgrabber.search.open')" shortcut="common.shortcuts.focus_indexer_search" :class="NAV_WIDE" color="neutral" variant="outline" data-testid="indexer-search-open" @click="emit('search')" />
        <NavbarAction icon="i-lucide-radar" :label="t('linkgrabber.actions.check_links')" :class="NAV_WIDE" color="neutral" variant="outline" :loading="props.checking" :disabled="!props.canCheck" @click="emit('check')" />
        <!-- The one solid button in this bar. Getting the reviewed links into the queue is what
             the LinkGrabber is for; adding and importing are how they arrive, and they read as
             the neutral pair they belong to. As `soft` beside a solid "Add links" this sat
             below the action that only fills the list it is meant to empty. -->
        <NavbarAction icon="i-lucide-list-end" :label="t('linkgrabber.actions.enqueue_all')" shortcut="common.shortcuts.enqueue_all" :disabled="!props.hasEntries || props.checking" :loading="props.enqueuing" @click="emit('enqueue', false)" />
        <NavbarAction icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" :hint="t('linkgrabber.actions.enqueue_paused_hint')" shortcut="common.shortcuts.enqueue_paused" :class="NAV_WIDE" color="neutral" variant="outline" :disabled="!props.hasEntries || props.checking" :loading="props.enqueuing" @click="emit('enqueue', true)" />
        <NavbarAction icon="i-lucide-list-x" :label="t('linkgrabber.actions.clear_all')" shortcut="common.shortcuts.clear_linkgrabber" :class="NAV_WIDE" color="error" variant="soft" :disabled="!props.hasEntries" @click="emit('clearAll')" />
        <UDropdownMenu :items="navbarMenu">
          <NavbarAction icon="i-lucide-ellipsis" :label="t('linkgrabber.actions.more')" class="@min-[40rem]:hidden" color="neutral" variant="outline" data-testid="linkgrabber-more" />
        </UDropdownMenu>
      </div>
    </template>
  </UDashboardNavbar>
</template>
