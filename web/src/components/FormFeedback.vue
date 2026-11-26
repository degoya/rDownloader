<script setup lang="ts">
/**
 * The outcome of a form's last action, above or under the form (RD-1120-15): the failure as an
 * error notice, the confirmation as a success notice, each with its icon (`design.md`, *Form
 * feedback*), the failure announced at once, the confirmation when the reader pauses
 * (`role`, WCAG 4.1.3). Classes given to the component go on each notice; `testid` names them
 * `<testid>-error` and `<testid>-message`.
 */
defineOptions({ inheritAttrs: false })

defineProps<{
  error?: string | null
  message?: string | null
  testid?: string
}>()
</script>

<template>
  <UAlert v-if="error" v-bind="$attrs" color="error" role="alert" icon="i-lucide-circle-alert" :description="error" :data-testid="testid && `${testid}-error`" />
  <UAlert v-if="message" v-bind="$attrs" color="success" role="status" icon="i-lucide-circle-check" :description="message" :data-testid="testid && `${testid}-message`" />
</template>
