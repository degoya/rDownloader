import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import en from '@/locales/en/linkgrabber.json'

import MediaOutputTemplateField from './MediaOutputTemplateField.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en } } })

const components = {
  UFormField: { props: ['label', 'description'], template: '<label v-bind="$attrs">{{ label }} {{ description }}<slot /></label>' },
  UInput: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template: '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)">'
  }
}

function mount(template: string | null, resolve: (value: string) => Promise<unknown>) {
  return render(MediaOutputTemplateField, {
    props: { template, resolve: resolve as never },
    global: { plugins: [i18n], components }
  })
}

describe('MediaOutputTemplateField', () => {
  it('shows the path the server resolved rather than guessing at one', async () => {
    // A second client-side evaluator would drift from the one the download uses, and the
    // whole point of the preview is that it is the real path.
    const resolve = vi.fn().mockResolvedValue({ relative_path: 'Studio/2026/Trailer.mp4', fields: ['title', 'uploader'] })
    mount('{uploader}/{upload_year}/{title}', resolve)
    await waitFor(() => expect(screen.getByTestId('media-output-preview').textContent).toContain('Studio/2026/Trailer.mp4'))
    expect(screen.getByText(/\{title\} \{uploader\}/)).toBeTruthy()
  })

  it('shows why a template was refused and does not commit it', async () => {
    const resolve = vi.fn().mockResolvedValue({ error: '`{shell}` is not a field a template can use' })
    const { emitted } = mount('{shell}', resolve)
    await waitFor(() => expect(screen.getByTestId('media-output-error')).toBeTruthy())
    expect(screen.getByTestId('media-output-error').textContent).toContain('not a field')
    expect(screen.queryByTestId('media-output-preview')).toBeNull()

    await fireEvent.blur(screen.getByTestId('media-output-input'))
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(2))
    expect(emitted().change).toBeUndefined()
  })

  it('commits a template that previewed cleanly', async () => {
    const resolve = vi.fn().mockResolvedValue({ relative_path: 'Trailer.mp4', fields: [] })
    const { emitted } = mount('', resolve)
    const input = screen.getByTestId('media-output-input')
    await fireEvent.update(input, '{title}')
    await fireEvent.blur(input)
    await waitFor(() => expect(emitted().change).toBeTruthy())
    expect((emitted().change as (string | null)[][])[0]?.[0]).toBe('{title}')
  })

  it('commits an emptied template as "no template" rather than as an empty string', async () => {
    const resolve = vi.fn().mockResolvedValue({ relative_path: 'clip.mp4', fields: [] })
    const { emitted } = mount('{title}', resolve)
    const input = screen.getByTestId('media-output-input')
    await fireEvent.update(input, '   ')
    await fireEvent.blur(input)
    await waitFor(() => expect(emitted().change).toBeTruthy())
    expect((emitted().change as (string | null)[][])[0]?.[0]).toBeNull()
  })
})
