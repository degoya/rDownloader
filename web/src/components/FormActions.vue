<script setup lang="ts">
/**
 * The action row every form ends with (RD-150-11).
 *
 * Sixty forms had written this row by hand, and it drifted the way the section header had: the
 * primary action right-aligned in two of them, "Cancel" spelled nine ways, a plus icon on a
 * button that saved an edit. The order is the standard in `design.md` (*Forms Share One Shape*):
 * the submit button first, then — only while a row is being edited — the icon-only cross that
 * leaves the edit, then whatever else the form offers. The row is a composition of `UButton`s,
 * not a replacement for them.
 */
import { useI18n } from 'vue-i18n'

const props = withDefaults(defineProps<{
  /** True while the form edits an existing row; the submit button then saves. */
  editing?: boolean
  /**
   * Offers the cross although nothing is being edited — for a draft the form holds that is not
   * an edit, such as a copy that cannot be created until a field changes. Follows `editing`
   * unless set.
   */
  cancellable?: boolean | undefined
  /** The label of the submit button while creating: "Create <thing>". */
  createLabel: string
  /** The icon of the submit button while creating. */
  createIcon?: string
  /** The label of the submit button while editing; "Save" unless the form says more. */
  saveLabel?: string | undefined
  loading?: boolean
  disabled?: boolean
}>(), {
  editing: false,
  cancellable: undefined,
  createIcon: 'i-lucide-plus',
  saveLabel: undefined,
  loading: false,
  disabled: false
})

const emit = defineEmits<{ cancel: [] }>()

defineSlots<{
  /** Further actions — a test, a preview — after the two the standard fixes. */
  default?(): unknown
}>()

const { t } = useI18n()
</script>

<template>
  <div class="flex flex-wrap items-center gap-2" data-form-actions>
    <UButton
      type="submit"
      :icon="props.editing ? 'i-lucide-save' : props.createIcon"
      :label="props.editing ? (props.saveLabel ?? t('common.actions.save')) : props.createLabel"
      :loading="props.loading"
      :disabled="props.disabled"
    />
    <UButton
      v-if="props.cancellable ?? props.editing"
      type="button"
      color="neutral"
      variant="ghost"
      icon="i-lucide-x"
      :aria-label="t('common.actions.cancel_edit')"
      :title="t('common.actions.cancel_edit')"
      @click="emit('cancel')"
    />
    <slot />
  </div>
</template>
