<script setup lang="ts">
/**
 * A day to type or to pick from a calendar (RD-1140-09).
 *
 * Nuxt UI's date field takes a day by its segments alone; the owner looked for a calendar under
 * "Service last seen alive" and found none. This is the form Nuxt UI documents for it: the
 * `UInputDate`, a calendar button in its `#trailing` slot and a `UCalendar` in a `UPopover`
 * behind that button. Both work on the one model, a `YYYY-MM-DD` day as the settings and the API
 * keep it (`utils/timeFields.ts`), the empty string for no day; a pick in the calendar closes it.
 * Week start and month names follow the interface's language, as the segments' order does.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { dateFieldValue, dayOf, type DateLike } from '@/utils/timeFields'

const props = withDefaults(defineProps<{
  /** The earliest day the field takes, `YYYY-MM-DD`. */
  min?: string
  /** The latest day the field takes, `YYYY-MM-DD`. */
  max?: string
  disabled?: boolean
}>(), {
  disabled: false
})

const day = defineModel<string>({ default: '' })

const { t, locale } = useI18n()
const open = ref(false)

const value = computed(() => dateFieldValue(day.value))
const minValue = computed(() => dateFieldValue(props.min))
const maxValue = computed(() => dateFieldValue(props.max))

function set(next: DateLike): void {
  day.value = dayOf(next)
}

function pick(next: DateLike): void {
  set(next)
  open.value = false
}
</script>

<template>
  <UInputDate
    :model-value="value"
    :min-value="minValue"
    :max-value="maxValue"
    :disabled="props.disabled"
    :locale="locale"
    @update:model-value="set"
  >
    <template #trailing>
      <UPopover v-model:open="open">
        <UButton
          color="neutral"
          variant="link"
          size="sm"
          icon="i-lucide-calendar"
          class="px-0"
          :aria-label="t('common.date_field.open_calendar')"
          :disabled="props.disabled"
        />
        <template #content>
          <UCalendar
            :model-value="value"
            :min-value="minValue"
            :max-value="maxValue"
            :disabled="props.disabled"
            :locale="locale"
            prevent-deselect
            class="p-2"
            @update:model-value="pick"
          />
        </template>
      </UPopover>
    </template>
  </UInputDate>
</template>
