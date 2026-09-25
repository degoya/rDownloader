export const NO_SELECTION = '__rdownloader_none__'

export function optionalSelection(value: string | null | undefined): string {
  return value ?? NO_SELECTION
}

export function selectionValue(value: string): string | null {
  return value === NO_SELECTION ? null : value
}
