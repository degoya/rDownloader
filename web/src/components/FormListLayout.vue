<script setup lang="ts">
/**
 * A form beside the list it feeds (RD-106-15).
 *
 * Every area that creates entries used to stack the form above the list. Pressing "edit" on a
 * row filled the form and changed its heading — both above the fold by then, so the list
 * looked unchanged and the edit went unnoticed. The form now stands in the left column and
 * the list in the right, so the heading, the buttons and the row being edited are on screen
 * together; below `lg` the columns stack again, form first, as they did.
 *
 * The breakpoint and the column ratio live here and nowhere else. A view that needs a
 * different split changes it for all of them, deliberately.
 */
defineProps<{
  /** Heading of the list column; absent when the list brings a section heading of its own. */
  listTitle?: string
  /** How many entries the list holds, shown beside its heading. */
  count?: number
}>()

defineSlots<{
  form(): unknown
  list(): unknown
  /** Controls that belong to the list as a whole — backup buttons, a filter. */
  'list-actions'?(): unknown
}>()
</script>

<template>
  <div class="grid gap-5 lg:grid-cols-2 lg:items-start">
    <div class="min-w-0">
      <slot name="form" />
    </div>
    <div class="min-w-0">
      <div v-if="listTitle" class="mb-3 flex items-center justify-between gap-2">
        <h3 class="text-sm font-semibold text-highlighted">{{ listTitle }}</h3>
        <div class="flex items-center gap-2">
          <slot name="list-actions" />
          <UBadge v-if="count !== undefined" color="neutral" variant="outline">{{ count }}</UBadge>
        </div>
      </div>
      <slot name="list" />
    </div>
  </div>
</template>
