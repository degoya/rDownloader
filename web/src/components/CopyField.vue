<script setup lang="ts">
/**
 * A value to copy: a read-only field and the button that copies it (RD-1110-13).
 *
 * Eight places — a token, a header, a command, a redirect address, a TOTP key — had written
 * this row by hand as a `<code>` beside a button, each a little differently, and only one of them
 * said on the button that the copy had happened. The value sits in a `UInput readonly` now, so it
 * can still be selected by hand where the clipboard is refused, and the button says "Copied" for
 * two seconds after a copy that worked. A copy that failed raises `useCopy`'s error toast; one
 * that worked emits `copied`, for a caller that has more to say than the button does.
 */
import { onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useCopy } from '@/composables/useCopy'

const props = withDefaults(defineProps<{
  value: string
  /** The button's label — "Copy token" —, also the field's accessible name. */
  label: string
  /** Only the icon on the button, the label as its name and tooltip; for a tight row. */
  iconOnly?: boolean
}>(), {
  iconOnly: false
})

const emit = defineEmits<{ copied: [] }>()

const { t } = useI18n()
const copyText = useCopy()
const copied = ref(false)
let reset: ReturnType<typeof setTimeout> | undefined

async function copy(): Promise<void> {
  if (!(await copyText(props.value))) return
  copied.value = true
  clearTimeout(reset)
  reset = setTimeout(() => { copied.value = false }, 2000)
  emit('copied')
}

onBeforeUnmount(() => clearTimeout(reset))
</script>

<template>
  <UFieldGroup class="w-full" data-copy-field>
    <UInput
      :model-value="props.value"
      readonly
      class="min-w-0 flex-1"
      :ui="{ base: 'font-mono text-xs' }"
      :aria-label="props.label"
    />
    <UButton
      :icon="copied ? 'i-lucide-copy-check' : 'i-lucide-copy'"
      :color="copied ? 'success' : 'neutral'"
      variant="outline"
      :label="props.iconOnly ? undefined : (copied ? t('common.copy.copied') : props.label)"
      :aria-label="props.iconOnly ? (copied ? t('common.copy.copied') : props.label) : undefined"
      :title="props.iconOnly ? (copied ? t('common.copy.copied') : props.label) : undefined"
      @click="copy"
    />
  </UFieldGroup>
</template>
