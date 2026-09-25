<script setup lang="ts">
import { computed } from 'vue'

import { timezoneOptions } from '@/utils/timezones'

/**
 * Searchable IANA time zone picker, used wherever a schedule needs a zone.
 *
 * These were plain text inputs, so a typo — or simply not knowing the exact IANA spelling —
 * produced a schedule that silently ran in the wrong zone.
 */
const model = defineModel<string>({ required: true })
const props = defineProps<{ disabled?: boolean, ariaLabel?: string }>()

const items = computed(() => timezoneOptions(model.value))
</script>

<template>
  <USelectMenu
    v-model="model"
    :items="items"
    :disabled="props.disabled"
    :aria-label="props.ariaLabel"
    icon="i-lucide-globe"
    class="w-full font-mono"
  />
</template>
