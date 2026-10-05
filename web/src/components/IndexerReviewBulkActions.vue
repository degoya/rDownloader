<script setup lang="ts">
import { useI18n } from 'vue-i18n'

/**
 * *Queue all* / *Dismiss all* of one indexer subscription's waiting hits (RD-1101-01).
 *
 * One component for the group's header and its footer, so the pair at the end of a group can
 * never differ from the pair at its top; the confirmations stay with the list that owns them.
 */
const { t } = useI18n()

const props = defineProps<{ busy: boolean }>()
const emit = defineEmits<{
  queueAll: []
  dismissAll: []
}>()
</script>

<template>
  <UButton
    size="xs"
    color="primary"
    variant="soft"
    icon="i-lucide-list-end"
    :label="t('linkgrabber.indexers.queue_all')"
    :loading="props.busy"
    @click="emit('queueAll')"
  />
  <UButton
    size="xs"
    color="neutral"
    variant="ghost"
    icon="i-lucide-x"
    :label="t('linkgrabber.indexers.dismiss_all')"
    :disabled="props.busy"
    @click="emit('dismissAll')"
  />
</template>
