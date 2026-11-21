import { ref } from 'vue'

/**
 * Whether a LinkGrabber package's menu offers handing its links to a remote-job provider
 * (RD-1440-07). A `ref`, like `showNzbHandOver`, so switching it in the settings changes the rows
 * already on screen. On by default; off hides the offer alone, and a link already marked keeps
 * its badge.
 */
export const showPackageHandOver = ref(true)

/** The switch the regenerated settings schema will carry. */
interface PackageHandOverSwitch {
  linkgrabber_hand_over_enabled?: boolean | null
}

/**
 * Takes any settings document: typed as `object` until the schema is regenerated, because the
 * generated `Settings` shares no key with the switch yet and TypeScript refuses such a weak match.
 */
export function setShowPackageHandOver(settings: object): void {
  showPackageHandOver.value = (settings as PackageHandOverSwitch).linkgrabber_hand_over_enabled !== false
}
