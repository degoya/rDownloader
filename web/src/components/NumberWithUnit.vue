<script setup lang="ts" generic="T extends number | null | undefined">
import { unitName } from '@/utils/units'

/**
 * A number field with its unit attached (RD-1140-08): a `UInputNumber` and an outline `UBadge`
 * in one `UFieldGroup`, so `MiB/s` stands at the number it belongs to, not at the far end of
 * the label row as the `UFormField` hint did. A size reads as the language names it (`Mio/s`).
 *
 * Every attribute but `class` goes to the number field — `min`, `max`, `required`, the format,
 * `increment decrement`, a test id; `class` sizes the group, the field fills it beside the unit.
 */
defineOptions({ inheritAttrs: false })
defineProps<{ unit: string }>()
const model = defineModel<T>()
</script>

<template>
  <UFieldGroup :class="['flex', $attrs.class]" data-number-unit>
    <UInputNumber v-model="model" v-bind="{ ...$attrs, class: undefined }" class="min-w-0 flex-1" />
    <UBadge color="neutral" variant="outline" :label="unitName(unit)" class="shrink-0 font-mono" />
  </UFieldGroup>
</template>
