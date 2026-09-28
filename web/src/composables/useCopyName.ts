import { useI18n } from 'vue-i18n'

import { duplicateName } from '@/utils/copyName'

/**
 * The name a duplicate gets, the same way in every list that offers one (RD-150-12).
 *
 * Category rules, subscriptions and site rules each passed their own "copy" suffix to
 * `duplicateName`, from three catalogue keys that said the same word. The suffix is one key now,
 * `common.copy_suffix`; what still differs per list is the length its server accepts, so that
 * stays a parameter. A name already taken counts up: "Films (copy)", "Films (copy 2)".
 */
export function useCopyName() {
  const { t } = useI18n()
  return (original: string, existingNames: Iterable<string>, maxLength: number): string =>
    duplicateName(original, existingNames, t('common.copy_suffix'), maxLength)
}
