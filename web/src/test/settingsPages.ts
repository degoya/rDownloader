/**
 * The settings pages are chunks the view fetches when one is shown (RD-1480-18). A test of the
 * view loads them up front, so its waits are for the page and not for the transform of its files.
 */
export async function loadSettingsPages(): Promise<void> {
  const pages = import.meta.glob('../components/settings/Settings*.vue')
  await Promise.all(Object.values(pages).map(load => load()))
}
