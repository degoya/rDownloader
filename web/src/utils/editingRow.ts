/**
 * How a list row shows that the form beside it is editing it (RD-1120-15), in the three shapes a
 * list has: a row in a box of its own turns its border primary, a row of a hairline-divided list
 * gets a primary bar on its left, and a row inside a card that already draws its edges gets a
 * primary outline. The row keeps its own padding.
 */
export function editingRowClass(editing: boolean, shape: 'box' | 'stripe' | 'outline' = 'box'): string {
  if (shape === 'box') return editing ? 'border border-primary' : 'border border-muted'
  if (!editing) return ''
  return shape === 'stripe' ? 'border-l-2 border-l-primary' : 'outline outline-1 outline-primary'
}
