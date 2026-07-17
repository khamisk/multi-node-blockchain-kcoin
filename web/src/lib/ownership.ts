import type { LeaderboardEntry } from '../types'

export const OWNERSHIP_MAP_WIDTH = 800
export const OWNERSHIP_MAP_HEIGHT = 450
const MAP_AREA_FILL = 0.22

export interface OwnershipVisualEntry {
  key: string
  label: string
  balance_atoms: string
  share_bps: number
  aggregate: boolean
  entry?: LeaderboardEntry
}

export interface PackedOwnershipEntry extends OwnershipVisualEntry {
  x: number
  y: number
  radius: number
}

/**
 * The visual covers the top 30 API entries and groups every remaining atom
 * into one "Other holders" entry.
 */
export function buildOwnershipVisualEntries(
  entries: LeaderboardEntry[],
  circulatingSupplyAtoms: string,
): OwnershipVisualEntry[] {
  const circulating = BigInt(circulatingSupplyAtoms)
  const top = entries.slice(0, 30)
  const represented = top.reduce((sum, entry) => sum + BigInt(entry.balance_atoms), 0n)
  const omitted = circulating > represented ? circulating - represented : 0n
  const visual: OwnershipVisualEntry[] = top.map((entry) => ({
    key: entry.address,
    label: entry.display_name ?? `Wallet #${entry.rank}`,
    balance_atoms: entry.balance_atoms,
    share_bps: entry.share_bps,
    aggregate: false,
    entry,
  }))

  if (omitted > 0n) {
    visual.push({
      key: 'other-holders',
      label: 'Other holders',
      balance_atoms: omitted.toString(),
      share_bps: circulating === 0n ? 0 : Number(omitted * 10_000n / circulating),
      aggregate: true,
    })
  }
  return visual
}

/**
 * A single common scale makes r² proportional to balance for every circle.
 * A per-circle minimum radius would make small holders look richer than they are.
 */
export function packOwnershipEntries(entries: OwnershipVisualEntry[]): PackedOwnershipEntry[] {
  const positive = entries.filter((entry) => BigInt(entry.balance_atoms) > 0n)
  const total = positive.reduce((sum, entry) => sum + Number(BigInt(entry.balance_atoms)), 0)
  if (total === 0) return []

  const commonScale = Math.sqrt(OWNERSHIP_MAP_WIDTH * OWNERSHIP_MAP_HEIGHT * MAP_AREA_FILL / Math.PI)
  const placed: PackedOwnershipEntry[] = []

  positive.forEach((entry, index) => {
    const radius = Math.sqrt(Number(BigInt(entry.balance_atoms)) / total) * commonScale
    let x = OWNERSHIP_MAP_WIDTH / 2
    let y = OWNERSHIP_MAP_HEIGHT / 2
    let found = index === 0

    for (let step = 1; !found && step < 40_000; step += 1) {
      const angle = step * 2.399963229728653
      const distance = Math.sqrt(step) * 1.28
      x = OWNERSHIP_MAP_WIDTH / 2 + Math.cos(angle) * distance
      y = OWNERSHIP_MAP_HEIGHT / 2 + Math.sin(angle) * distance * 0.72
      const within = x - radius >= 3 && x + radius <= OWNERSHIP_MAP_WIDTH - 3
        && y - radius >= 3 && y + radius <= OWNERSHIP_MAP_HEIGHT - 3
      const clear = placed.every((other) => Math.hypot(x - other.x, y - other.y) >= radius + other.radius + 2)
      found = within && clear
    }

    placed.push({ ...entry, x, y, radius })
  })
  return placed
}
