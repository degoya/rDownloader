/**
 * Whether a host name only reaches this computer: `localhost` and its subdomains, `127.0.0.0/8`
 * and `::1`, bracketed as `location.hostname` gives it or bare (RD-1390-05). The pairing card
 * copies the address the interface was opened at, which another computer cannot use when it is
 * one of these.
 */
export function isLoopbackHost(hostname: string): boolean {
  const host = hostname.trim().toLowerCase().replace(/\.$/, '')
  if (host === 'localhost' || host.endsWith('.localhost')) return true
  if (host === '[::1]' || host === '::1') return true
  const octets = host.split('.')
  return octets.length === 4
    && octets[0] === '127'
    && octets.every(octet => /^\d{1,3}$/.test(octet) && Number(octet) <= 255)
}
