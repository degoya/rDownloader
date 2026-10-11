import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { AutomationSchedule } from '@/api/types'
import en from '@/locales/en/automation.json'
import { mountComponent } from '@/test/mount'

import AutomationTriggerCard from './AutomationTriggerCard.vue'

/** A menu or select as a `<select>` that answers with the chosen item's value. */
const select = {
  props: ['modelValue', 'items'],
  emits: ['update:modelValue'],
  template:
    '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)">' +
    '<option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
}

function mount(trigger: string, schedule: AutomationSchedule, packageActionOnSchedule = false) {
  const schedules: AutomationSchedule[] = []
  mountComponent(AutomationTriggerCard, {
    messages: { automation: en },
    props: {
      trigger,
      schedule,
      packageActionOnSchedule,
      triggerOptions: [
        { value: 'download_completed', label: en.trigger.download_completed },
        { value: 'schedule', label: en.trigger.schedule }
      ],
      'onUpdate:schedule': (value: AutomationSchedule) => schedules.push(value)
    },
    stubs: { USelectMenu: select, USelect: select }
  })
  return { schedules }
}

describe('AutomationTriggerCard (RD-1240-10)', () => {
  it('asks for a schedule only for the time trigger', () => {
    mount('download_completed', { kind: 'interval', minutes: 60 })
    expect(screen.queryByText(en.schedule.heading)).toBeNull()
  })

  it('switches between an interval and a cron line, each with a working default', async () => {
    const { schedules } = mount('schedule', { kind: 'interval', minutes: 60 })
    // The form field is a stub here: its label is text, its help an attribute.
    expect(screen.getByText(en.schedule.minutes)).toBeTruthy()
    expect(document.querySelector(`[description="${en.schedule.help}"]`)).toBeTruthy()
    await fireEvent.update(screen.getByLabelText(en.schedule.heading), 'cron')
    expect(schedules).toEqual([{ kind: 'cron', expression: '0 6 * * *' }])
  })

  it('names a package action a schedule cannot run', () => {
    mount('schedule', { kind: 'cron', expression: '0 6 * * *' }, true)
    expect(screen.getByText(en.schedule.no_package)).toBeTruthy()
    expect(screen.getByText(en.schedule.expression)).toBeTruthy()
  })
})
