export const ATOMS_PER_KCOIN = 1_000_000n

export function formatInteger(value: string | bigint): string {
  try {
    return (typeof value === 'bigint' ? value : BigInt(value)).toLocaleString('en-US')
  } catch {
    return String(value)
  }
}

export function formatKcoin(atoms: string | bigint, maximumFractionDigits = 6): string {
  const value = typeof atoms === 'bigint' ? atoms : BigInt(atoms || '0')
  const whole = value / ATOMS_PER_KCOIN
  const fraction = (value % ATOMS_PER_KCOIN).toString().padStart(6, '0').replace(/0+$/, '')
  const shown = fraction.slice(0, maximumFractionDigits)
  return `${whole.toLocaleString('en-US')}${shown ? `.${shown}` : ''}`
}

export function parseKcoin(input: string): string {
  if (!/^\d+(?:\.\d{0,6})?$/.test(input.trim())) throw new Error('Use up to 6 decimal places.')
  const [whole, fraction = ''] = input.trim().split('.')
  return (BigInt(whole) * ATOMS_PER_KCOIN + BigInt(fraction.padEnd(6, '0') || '0')).toString()
}

export function shortHash(value: string, leading = 8, trailing = 6): string {
  if (!value) return '—'
  if (value.length <= leading + trailing + 1) return value
  return `${value.slice(0, leading)}…${value.slice(-trailing)}`
}

export function timeAgo(timestamp: string): string {
  const seconds = Math.max(0, Math.floor((Date.now() - new Date(timestamp).getTime()) / 1000))
  if (seconds < 5) return 'now'
  if (seconds < 60) return `${seconds}s ago`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ago`
  return `${Math.floor(minutes / 60)}h ago`
}

export function percentFromBps(bps: number): string {
  return `${(bps / 100).toFixed(bps % 100 === 0 ? 0 : 2)}%`
}

export function encodeBase64(bytes: Uint8Array): string {
  let binary = ''
  bytes.forEach((byte) => (binary += String.fromCharCode(byte)))
  return btoa(binary)
}

export function decodeBase64(value: string): Uint8Array {
  const binary = atob(value)
  return Uint8Array.from(binary, (char) => char.charCodeAt(0))
}

export function deterministicHex(seed: string, length = 64): string {
  let state = 2166136261
  for (let index = 0; index < seed.length; index += 1) {
    state ^= seed.charCodeAt(index)
    state = Math.imul(state, 16777619)
  }
  let output = ''
  while (output.length < length) {
    state ^= state << 13
    state ^= state >>> 17
    state ^= state << 5
    output += (state >>> 0).toString(16).padStart(8, '0')
  }
  return output.slice(0, length)
}
