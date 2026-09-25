<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { HosterCount } from '@/composables/useHiddenHosters'

/**
 * The LinkGrabber's hoster quick filter (RD-130-21).
 *
 * The wrapping chip row of `design.md`, as a set of switches rather than one choice: every
 * hoster in the list with its number of links, pressed while it is shown. Beside it the line
 * that says how much the hidden hosters hold back and brings everything back in one click —
 * a list that shows part of itself says which part.
 */
const props = defineProps<{
  hosters: HosterCount[]
  /** Links the hidden hosters take off the screen; a mirror of a shown link is not one. */
  hiddenLinks: number
  /** How many hosters those links belong to. */
  hiddenHosters: number
  busy: boolean
}>()
const emit = defineEmits<{
  /** Hide a hoster (`true`) or show it again (`false`). */
  toggle: [hoster: string, hide: boolean]
  'show-all': []
}>()
const { t } = useI18n()
</script>

<template>
  <!-- One hoster is nothing to choose between; the row returns as soon as a second arrives, and
       it stays while anything is hidden, so the way back is always on screen. -->
  <div v-if="props.hosters.length > 1 || props.hiddenLinks" class="flex flex-col gap-1.5">
    <div class="flex flex-wrap items-center gap-2" role="group" :aria-label="t('linkgrabber.hosters.label')" :title="t('linkgrabber.hosters.hint')">
      <UButton
        v-for="entry in props.hosters"
        :key="entry.hoster"
        class="max-w-full"
        size="xs"
        color="neutral"
        :variant="entry.hidden ? 'ghost' : 'outline'"
        :icon="entry.hidden ? 'i-lucide-eye-off' : 'i-lucide-eye'"
        :aria-pressed="!entry.hidden"
        :title="entry.hidden ? t('linkgrabber.hosters.show', { host: entry.hoster }) : t('linkgrabber.hosters.hide', { host: entry.hoster })"
        :disabled="props.busy"
        @click="emit('toggle', entry.hoster, !entry.hidden)"
      >
        <span class="truncate font-mono" :class="entry.hidden ? 'text-muted line-through' : ''">{{ entry.hoster }}</span>
        <UBadge size="xs" color="neutral" variant="subtle" class="font-mono">{{ entry.count }}</UBadge>
      </UButton>
    </div>
    <p v-if="props.hiddenLinks" class="flex flex-wrap items-center gap-2 text-xs text-muted" role="status">
      <UIcon name="i-lucide-eye-off" class="size-3.5" aria-hidden="true" />
      <span>{{ t('linkgrabber.hosters.summary', {
        links: t('common.units.link', { count: props.hiddenLinks }, props.hiddenLinks),
        hosters: t('linkgrabber.hosters.hosters', { count: props.hiddenHosters }, props.hiddenHosters)
      }) }}</span>
      <UButton size="xs" color="primary" variant="link" :label="t('linkgrabber.hosters.show_all')" :disabled="props.busy" @click="emit('show-all')" />
    </p>
  </div>
</template>
