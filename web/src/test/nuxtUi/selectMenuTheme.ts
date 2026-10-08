/**
 * `#build/ui/select-menu` and `#build/ui/input` without their classes: the slots the real select
 * menu and its search field ask for (`SearchableSelect.test.ts`).
 */
const SLOTS = [
  'root', 'base', 'leading', 'leadingIcon', 'leadingAvatar', 'leadingAvatarSize', 'trailing', 'trailingIcon',
  'trailingClear', 'value', 'placeholder', 'arrow', 'content', 'focusScope', 'input', 'empty', 'viewport', 'group',
  'label', 'separator', 'item', 'itemLeadingIcon', 'itemLeadingAvatar', 'itemLeadingAvatarSize', 'itemLeadingChip',
  'itemLeadingChipSize', 'itemWrapper', 'itemLabel', 'itemDescription', 'itemTrailing', 'itemTrailingIcon'
]

export default { slots: Object.fromEntries(SLOTS.map(slot => [slot, ''])) }
