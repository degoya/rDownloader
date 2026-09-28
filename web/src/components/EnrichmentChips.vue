<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { EnrichmentField } from '@/api/types'
import { formatMoment } from '@/utils/format'

/**
 * The chips an enricher plugin's fields become, under a package or a link (RD-107-02,
 * RD-150-19).
 *
 * The queue and the LinkGrabber showed the same row twice; this is the one copy. Each chip
 * carries its source and its age in the title, because a value the application did not resolve
 * itself has to stay recognisable as somebody else's answer — and a stale one as stale. The
 * label is the field name's last segment, translated where the interface knows it and printed
 * as it is where it does not, so a plugin's own field is never hidden for want of a word.
 */
const props = defineProps<{
  fields: EnrichmentField[]
}>()

const { t } = useI18n()

/** Suffixes with a catalogue entry under `common.enrichment.fields`. */
const KNOWN_FIELDS = new Set(['title', 'series', 'season', 'episode', 'year', 'rating', 'genre', 'runtime'])

function label(name: string): string {
  const suffix = name.split('.').pop() ?? name
  return KNOWN_FIELDS.has(suffix) ? t(`common.enrichment.fields.${suffix}`) : suffix
}
</script>

<template>
  <UBadge
    v-for="field in props.fields"
    :key="`${field.plugin_id}:${field.name}`"
    color="neutral"
    variant="subtle"
    size="sm"
    class="font-mono"
    data-testid="enrichment-chip"
    :title="t('common.enrichment.source', { name: field.name, at: formatMoment(field.fetched_at) })"
  >{{ label(field.name) }}: {{ field.value }}</UBadge>
</template>
