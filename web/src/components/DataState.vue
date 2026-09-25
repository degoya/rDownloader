<script setup lang="ts">
/**
 * What a fetched area shows while it has nothing to show yet (RD-104-07).
 *
 * Wraps the place where an empty state used to sit unconditionally. Exactly one of three
 * things renders: the loading surface while the first fetch is in flight, the failure when it
 * did not arrive, or the caller's own empty state — and only once the fetch has settled and
 * the list really is empty. When there is content, nothing renders at all.
 *
 * The loading surface is the 28 px signal grid `design.md` names as the motif for loading
 * surfaces, with pulse bars standing in for the rows that are coming. Deliberately not
 * `USkeleton`: the bars are three divs, they need no component stub in a test, and their
 * shape belongs to the list, not to a design-system primitive.
 */
import { useI18n } from 'vue-i18n'

const props = withDefaults(
  defineProps<{
    /** True while the first fetch is in flight. */
    loading?: boolean | undefined
    /** The failure message of the fetch, or `null`. Takes precedence over `empty`. */
    error?: string | null | undefined
    /** True when the fetch has settled and produced nothing. */
    empty?: boolean | undefined
    /** How many placeholder rows to draw; roughly what the list usually holds. */
    rows?: number | undefined
    /** `inline` drops the framed surface for lists that live inside another box. */
    variant?: 'panel' | 'inline' | undefined
    /**
     * What is being waited for, in place of the generic word, where the reader would otherwise
     * not know why a control is missing (RD-120-51).
     */
    label?: string | null | undefined
  }>(),
  { loading: false, error: null, empty: false, rows: 3, variant: 'panel', label: null }
)

const { t } = useI18n()
</script>

<template>
  <div
    v-if="props.loading"
    role="status"
    aria-live="polite"
    :class="props.variant === 'panel' ? 'signal-grid space-y-2 border border-dashed border-muted p-5' : 'space-y-2'"
  >
    <div
      v-for="row in props.rows"
      :key="row"
      aria-hidden="true"
      class="h-3 animate-pulse bg-elevated"
      :class="row === props.rows ? 'w-2/3' : 'w-full'"
    />
    <p class="pt-1 text-center text-xs text-muted">{{ props.label ?? t('common.data.loading') }}</p>
  </div>
  <p
    v-else-if="props.error"
    role="alert"
    :class="props.variant === 'panel'
      ? 'border border-dashed border-error p-5 text-center text-sm text-error'
      : 'text-xs text-error'"
  >
    {{ props.error }}
  </p>
  <!-- A wrapper, not the bare slot: a slot at the root is a fragment, and a fragment inherits no
       attributes, so a caller's `class="p-5"` or `mt-4` reached the loading and failure states but
       never the empty one — the wizard's empty storage-root box sat without padding (RD-120-48). -->
  <div v-else-if="props.empty">
    <slot />
  </div>
</template>
