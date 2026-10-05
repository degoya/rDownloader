<script setup lang="ts">
/**
 * The colour field of the category form: a picker and the hex value it stands for.
 *
 * The picker has no keyboard operation and names no value, so the colour is also a field of its
 * own: typed exactly, reached by Tab, refused by the form unless it is a whole #rrggbb. The
 * picker only ever sees a whole colour (RA-WEB-02).
 */
import { useI18n } from 'vue-i18n'

const color = defineModel<string>({ required: true })
const { t } = useI18n()

/** What the service takes as a category colour; the hex field holds to it (RA-WEB-02). */
const HEX_COLOR = /^#[0-9a-fA-F]{6}$/
</script>

<template>
  <UFormField :label="t('routing.category.color_label')" :description="t('routing.category.color_description')">
    <div class="flex items-center gap-2">
      <UPopover>
        <UButton
          color="neutral"
          variant="outline"
          :aria-label="t('routing.category.color_pick', { color })"
          data-testid="category-color"
        >
          <span class="size-4 shrink-0 border border-muted" :style="{ backgroundColor: color }" />
        </UButton>
        <template #content>
          <UColorPicker
            :model-value="HEX_COLOR.test(color) ? color : undefined"
            class="p-2"
            @update:model-value="(value?: string) => { if (value) color = value }"
          />
        </template>
      </UPopover>
      <UInput
        v-model="color"
        required
        pattern="#[0-9a-fA-F]{6}"
        maxlength="7"
        class="w-32 font-mono"
        placeholder="#38BDF8"
        :title="t('routing.category.color_format')"
        :aria-label="t('routing.category.color_hex')"
        data-testid="category-color-hex"
      />
    </div>
  </UFormField>
</template>
