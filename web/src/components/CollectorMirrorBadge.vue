<script setup lang="ts">
/**
 * The mirror group a LinkGrabber row stands for, and how sure it is (RD-110-19).
 *
 * The three sources are not equally strong, so the badge does not only change colour — it
 * changes the noun. `5 mirrors` is a statement; `5 possible mirrors` is a proposal, and
 * somebody who does not see the difference in colour still reads the difference in the word.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { MirrorGroup } from '@/utils/mirrorGroups'

const props = defineProps<{ group: MirrorGroup }>()

const { t } = useI18n()

const mirrorLabel = computed(() => {
  const count = props.group.members.length
  return props.group.source === 'name'
    ? t('linkgrabber.mirror.proposed', { count }, count)
    : t('linkgrabber.mirror.mirrors', { count }, count)
})
const mirrorHint = computed(() => {
  const group = props.group
  const evidence = t(`linkgrabber.mirror.hint_${group.source}`)
  return group.onlineCount === 0
    ? `${evidence}\n${t('linkgrabber.mirror.all_offline_hint', { count: group.members.length })}`
    : `${evidence}\n${t('linkgrabber.mirror.online_of', { online: group.onlineCount, total: group.members.length })}`
})
const mirrorColor = computed<'primary' | 'neutral' | 'warning'>(() => {
  switch (props.group.source) {
    case 'declared': return 'primary'
    case 'name': return 'warning'
    default: return 'neutral'
  }
})
/** The proposal wears a dashed edge as well as its own word, so it reads as unfinished. */
const mirrorClass = computed(() => props.group.source === 'name' ? 'border border-dashed' : '')
</script>

<template>
  <UBadge
    :color="mirrorColor"
    :variant="props.group.source === 'declared' ? 'subtle' : 'soft'"
    size="sm"
    class="shrink-0"
    :class="mirrorClass"
    :icon="props.group.source === 'name' ? 'i-lucide-circle-help' : 'i-lucide-layers'"
    :title="mirrorHint"
  >{{ mirrorLabel }}</UBadge>
</template>
