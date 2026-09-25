<script setup lang="ts">
/**
 * The eyebrow, heading and description every section of this interface opens with.
 *
 * The block was written out by hand 73 times across 49 files, and with nothing owning it, it
 * drifted: the heading had four spellings and the description twelve, mixing `mt-1` with `mt-2`,
 * `text-sm leading-6` with `text-xs leading-5`, and `max-w-3xl` with `max-w-prose` with no rule
 * behind any of it. Two headings had even lost `font-semibold` while the other fifty-seven kept
 * it. This is the same failure `design.md` records for the subscriptions list: a visual decision
 * with no home does not stay decided.
 *
 * Three levels, and they are a real hierarchy rather than a size picker — a settings tab header,
 * a card inside it, a section inside the card. The description width is fixed per level because
 * line length is a readability rule, not a per-site choice.
 *
 * Anything a section needs beside the heading — a switch, a badge, a button — stays in the
 * caller's own flex row; this component is the text block, not the bar it may sit in.
 */
const props = withDefaults(
  defineProps<{
    /** The small mono label above the heading. */
    eyebrow: string
    /** The heading itself. */
    title: string
    /** The sentence under it, when the heading does not say enough on its own. */
    description?: string | undefined
    /** Tab header, card, or a section inside a card. */
    level?: 'page' | 'card' | 'sub' | undefined
  }>(),
  { description: undefined, level: 'card' }
)

const HEADINGS = {
  page: 'text-xl',
  card: 'text-lg',
  sub: 'text-base'
} as const

const DESCRIPTIONS = {
  page: 'mt-2 max-w-3xl text-sm leading-6',
  card: 'mt-2 max-w-prose text-sm leading-6',
  sub: 'mt-1 max-w-prose text-xs leading-5'
} as const
</script>

<template>
  <div>
    <p class="eyebrow">{{ props.eyebrow }}</p>
    <component
      :is="props.level === 'page' ? 'h2' : 'h3'"
      :class="['mt-1 font-semibold text-highlighted', HEADINGS[props.level]]"
    >
      {{ props.title }}
    </component>
    <p v-if="props.description || $slots.description" :class="['text-muted', DESCRIPTIONS[props.level]]">
      <slot name="description">{{ props.description }}</slot>
    </p>
  </div>
</template>
