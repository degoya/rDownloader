<script setup lang="ts">
import { computed } from 'vue'

import { shortcutKeys, withShortcut } from '@/composables/shortcutDefinitions'

/**
 * One action of the Downloads or LinkGrabber navbar (RD-1260-06): an icon, its name the
 * `aria-label` and the tooltip, the key that runs it in parentheses behind the name — "Add links
 * (A)". The key comes from the shortcut catalogue, never written here. Colour, variant, state and
 * the click go through to the `UButton`, so a menu can wrap it as its trigger.
 */
const props = defineProps<{
  icon: string
  /** The action's name. */
  label: string
  /** A tooltip that says more than the name; the key is added to it all the same. */
  hint?: string
  /** The catalogue entry of the key, by its `descriptionKey` (`shortcutDefinitions.ts`). */
  shortcut?: string
}>()

const keys = computed(() => props.shortcut ? shortcutKeys(props.shortcut) : [])
const title = computed(() => withShortcut(props.hint ?? props.label, props.shortcut))
</script>

<template>
  <UButton :icon="props.icon" :aria-label="props.label" :title="title" :aria-keyshortcuts="keys.length ? keys.join('+') : undefined" />
</template>
