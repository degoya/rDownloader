<script setup lang="ts">
/**
 * A row of hairline-divided stat tiles — eyebrow, figure, hint — as the queue summary, the
 * statistics and the system facts show them (RD-1120-15). The caller sets the grid columns
 * through `class`; `surface` keeps the system facts on the page background, the others sit on
 * the elevated one. A tile whose figure carries markup fills the `value` slot.
 */
interface StatTile {
  key: string
  label: string
  value: string
  hint: string
  /** False for a figure that is a name rather than a number (the product's own). */
  numeric?: boolean
}

withDefaults(defineProps<{
  tiles: StatTile[]
  as?: string
  surface?: 'elevated' | 'default'
}>(), { as: 'div', surface: 'elevated' })
</script>

<template>
  <component :is="as" class="grid gap-px border border-muted bg-muted">
    <div v-for="tile in tiles" :key="tile.key" :class="surface === 'default' ? 'bg-default p-5' : 'bg-elevated p-4'" :data-tile="tile.key">
      <p class="eyebrow">{{ tile.label }}</p>
      <p class="mt-2 text-lg text-highlighted" :class="{ numeric: tile.numeric !== false }">
        <slot name="value" :tile="tile">{{ tile.value }}</slot>
      </p>
      <p class="mt-1 text-xs text-muted">{{ tile.hint }}</p>
    </div>
  </component>
</template>
