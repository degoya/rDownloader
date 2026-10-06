<script setup lang="ts">
/**
 * The drag handle of a reorderable row (`design.md`, *A drag starts at the handle, and only
 * there*; RD-1120-14).
 *
 * A native `<button>` carrying `draggable="true"`, as `design.md` describes it, built here once
 * for the five lists that reorder (queue file and package, LinkGrabber link and package, NZB
 * import). The arrow keys move the row one step, the keyboard equivalent of the drag. The
 * handle's place in its row's grid comes in as a class from the caller.
 */
const props = defineProps<{
  /** Names both the drag and the keys; it is the handle's title and accessible name. */
  label: string
}>()

const emit = defineEmits<{
  dragstart: []
  move: [delta: -1 | 1]
}>()
</script>

<template>
  <button
    type="button"
    class="cursor-grab select-none text-muted"
    data-row-handle
    draggable="true"
    :title="props.label"
    :aria-label="props.label"
    @dragstart.stop="emit('dragstart')"
    @keydown.up.prevent="emit('move', -1)"
    @keydown.down.prevent="emit('move', 1)"
  >
    <UIcon name="i-lucide-grip-vertical" class="size-4" />
  </button>
</template>
