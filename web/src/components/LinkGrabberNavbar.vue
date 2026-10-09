<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

/** The LinkGrabber's navbar: its buttons, each with its key, and on a phone one menu (1.8.1). */
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
 * The navbar measures its own width (`@container`), not the window's: beside the sidebar the
 * labelled row needs about 1300 px, 1500 with the keys, since the indexer search joined it
 * (RD-1230-02); below that it ran over the title and the sidebar toggle, and on a phone it ran
 * off the screen. So the key hints go first, then the labels — every button
 * keeps its name as `aria-label` and `title` — and on a phone what is neither adding nor
 * enqueuing moves into one menu, its keys shown there.
 */
const NAV_KBD = 'hidden @min-[94rem]:inline-flex'
const NAV_LABEL = { label: 'hidden @min-[82rem]:inline' }
const NAV_WIDE = 'hidden @min-[40rem]:inline-flex'
const navbarMenu = computed(() => [[
  { label: t('linkgrabber.actions.import_files'), icon: 'i-lucide-file-up', kbds: ['n'], onSelect: () => emit('import') },
  { label: t('linkgrabber.nzb.history.title'), icon: 'i-lucide-history', onSelect: () => emit('history') },
  { label: t('linkgrabber.search.open'), icon: 'i-lucide-search', kbds: ['f'], onSelect: () => emit('search') },
  { label: t('linkgrabber.actions.check_links'), icon: 'i-lucide-radar', disabled: !props.canCheck, onSelect: () => emit('check') },
  { label: t('linkgrabber.actions.enqueue_paused'), icon: 'i-lucide-pause', kbds: ['w'], disabled: !props.hasEntries || props.checking, onSelect: () => emit('enqueue', true) }
], [
  { label: t('linkgrabber.actions.clear_all'), icon: 'i-lucide-list-x', color: 'error' as const, kbds: ['r'], disabled: !props.hasEntries, onSelect: () => emit('clearAll') }
]])
</script>

<template>
  <UDashboardNavbar :title="t('linkgrabber.title')" :ui="{ root: '@container' }">
    <template #leading><UDashboardSidebarCollapse /></template>
    <template #right>
      <div data-tour="grabber-add" class="flex items-center gap-2">
        <UButton icon="i-lucide-plus" :label="t('linkgrabber.actions.add_links')" :aria-label="t('linkgrabber.actions.add_links')" :title="t('linkgrabber.actions.add_links')" :ui="NAV_LABEL" color="neutral" variant="outline" @click="emit('add')">
          <template #trailing><UKbd value="a" :class="NAV_KBD" /></template>
        </UButton>
        <UButton icon="i-lucide-file-up" :label="t('linkgrabber.actions.import_files')" :aria-label="t('linkgrabber.actions.import_files')" :title="t('linkgrabber.actions.import_files')" :ui="NAV_LABEL" :class="NAV_WIDE" color="neutral" variant="outline" :loading="props.importing" @click="emit('import')">
          <template #trailing><UKbd value="n" :class="NAV_KBD" /></template>
        </UButton>
        <UButton icon="i-lucide-history" :class="NAV_WIDE" color="neutral" variant="outline" :aria-label="t('linkgrabber.nzb.history.title')" :title="t('linkgrabber.nzb.history.title')" @click="emit('history')" />
        <UButton icon="i-lucide-search" :label="t('linkgrabber.search.open')" :aria-label="t('linkgrabber.search.open')" :title="t('linkgrabber.search.open')" :ui="NAV_LABEL" :class="NAV_WIDE" color="neutral" variant="outline" data-testid="indexer-search-open" @click="emit('search')">
          <template #trailing><UKbd value="f" :class="NAV_KBD" /></template>
        </UButton>
        <UButton icon="i-lucide-radar" :label="t('linkgrabber.actions.check_links')" :aria-label="t('linkgrabber.actions.check_links')" :title="t('linkgrabber.actions.check_links')" :ui="NAV_LABEL" :class="NAV_WIDE" color="neutral" variant="outline" :loading="props.checking" :disabled="!props.canCheck" @click="emit('check')" />
        <!-- The one solid button in this bar. Getting the reviewed links into the queue is what
             the LinkGrabber is for; adding and importing are how they arrive, and they read as
             the neutral pair they belong to. As `soft` beside a solid "Add links" this sat
             below the action that only fills the list it is meant to empty. -->
        <UButton icon="i-lucide-list-end" :label="t('linkgrabber.actions.enqueue_all')" :aria-label="t('linkgrabber.actions.enqueue_all')" :title="t('linkgrabber.actions.enqueue_all')" :ui="NAV_LABEL" :disabled="!props.hasEntries || props.checking" :loading="props.enqueuing" @click="emit('enqueue', false)">
          <template #trailing><UKbd value="e" :class="NAV_KBD" /></template>
        </UButton>
        <UButton icon="i-lucide-pause" :label="t('linkgrabber.actions.enqueue_paused')" :aria-label="t('linkgrabber.actions.enqueue_paused')" :ui="NAV_LABEL" :class="NAV_WIDE" color="neutral" variant="outline" :title="t('linkgrabber.actions.enqueue_paused_hint')" :disabled="!props.hasEntries || props.checking" :loading="props.enqueuing" @click="emit('enqueue', true)">
          <template #trailing><UKbd value="w" :class="NAV_KBD" /></template>
        </UButton>
        <UButton icon="i-lucide-list-x" :label="t('linkgrabber.actions.clear_all')" :aria-label="t('linkgrabber.actions.clear_all')" :title="t('linkgrabber.actions.clear_all')" :ui="NAV_LABEL" :class="NAV_WIDE" color="error" variant="soft" :disabled="!props.hasEntries" @click="emit('clearAll')">
          <template #trailing><UKbd value="r" :class="NAV_KBD" /></template>
        </UButton>
        <UDropdownMenu :items="navbarMenu">
          <UButton icon="i-lucide-ellipsis" class="@min-[40rem]:hidden" color="neutral" variant="outline" :aria-label="t('linkgrabber.actions.more')" :title="t('linkgrabber.actions.more')" data-testid="linkgrabber-more" />
        </UDropdownMenu>
      </div>
    </template>
  </UDashboardNavbar>
</template>
