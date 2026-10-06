<script setup lang="ts">
/**
 * One line that points from a setting to the one it works together with on another page
 * (RD-1120-23): "See also  Network › Proxy profiles". The target is a search anchor, so the
 * link follows a card that moves; its text is the page's name and the card's or field's title,
 * from the same registry, unless `titleKey` names the field more exactly.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSettingsLink } from '@/composables/useSettingsLink'

const props = defineProps<{
  /** The `data-settings-anchor` of the card or field, as `settingsSearch.ts` lists it. */
  anchor: string
  /** The words before the link; "See also" when none are given. */
  lead?: string
  /** An i18n key for the target's title instead of the registry's. */
  titleKey?: string
}>()

const { t } = useI18n()
const { entry, to, pageLabelKey, reveal } = useSettingsLink(() => props.anchor)
const label = computed(() => {
  const title = t(props.titleKey ?? entry.value?.titleKey ?? '')
  return pageLabelKey.value ? `${t(pageLabelKey.value)} › ${title}` : title
})
</script>

<template>
  <p v-if="to" class="flex flex-wrap items-center gap-x-1.5 text-xs text-muted" data-settings-link>
    <span>{{ lead ?? t('settings.cross_link.see_also') }}</span>
    <ULink :to="to" class="inline-flex items-center gap-1 font-medium text-primary" :data-anchor="anchor" @click="reveal">
      <UIcon name="i-lucide-arrow-right" class="size-3.5 shrink-0" />
      {{ label }}
    </ULink>
  </p>
</template>
