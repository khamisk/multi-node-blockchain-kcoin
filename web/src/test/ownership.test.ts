import { describe, expect, it } from 'vitest'
import { OWNERSHIP_MAP_HEIGHT, OWNERSHIP_MAP_WIDTH, buildOwnershipVisualEntries, packOwnershipEntries } from '../lib/ownership'
import type { LeaderboardEntry } from '../types'

function entry(rank: number, balance: bigint): LeaderboardEntry {
  return {
    rank,
    address: `kcoin1holder${rank}`,
    balance_atoms: balance.toString(),
    share_bps: Number(balance),
    transaction_count: '1',
  }
}

describe('ownership visualization', () => {
  it('shows at most 30 wallets and aggregates every omitted atom', () => {
    const entries = Array.from({ length: 32 }, (_, index) => entry(index + 1, BigInt(100 - index)))
    const circulating = entries.reduce((sum, item) => sum + BigInt(item.balance_atoms), 0n) + 17n
    const visual = buildOwnershipVisualEntries(entries, circulating.toString())
    const aggregate = visual.find((item) => item.aggregate)

    expect(visual.filter((item) => !item.aggregate)).toHaveLength(30)
    expect(aggregate?.balance_atoms).toBe((BigInt(entries[30].balance_atoms) + BigInt(entries[31].balance_atoms) + 17n).toString())
    expect(visual.reduce((sum, item) => sum + BigInt(item.balance_atoms), 0n)).toBe(circulating)
  })

  it('keeps circle area proportional to balance with no hard minimum', () => {
    const packed = packOwnershipEntries([
      { key: 'large', label: 'Large', balance_atoms: '100', share_bps: 9900, aggregate: false },
      { key: 'small', label: 'Small', balance_atoms: '1', share_bps: 100, aggregate: false },
    ])

    const large = packed.find((item) => item.key === 'large')!
    const small = packed.find((item) => item.key === 'small')!
    expect((large.radius ** 2) / (small.radius ** 2)).toBeCloseTo(100, 10)
    expect(small.radius).toBeLessThan(large.radius / 9)
  })

  it('packs a top-30-plus-aggregate dataset without overlaps or clipping', () => {
    const entries = Array.from({ length: 30 }, (_, index) => entry(index + 1, BigInt((31 - index) ** 2)))
    const visual = buildOwnershipVisualEntries(entries, (entries.reduce((sum, item) => sum + BigInt(item.balance_atoms), 0n) + 300n).toString())
    const packed = packOwnershipEntries(visual)

    for (const [index, circle] of packed.entries()) {
      expect(circle.x - circle.radius).toBeGreaterThanOrEqual(0)
      expect(circle.x + circle.radius).toBeLessThanOrEqual(OWNERSHIP_MAP_WIDTH)
      expect(circle.y - circle.radius).toBeGreaterThanOrEqual(0)
      expect(circle.y + circle.radius).toBeLessThanOrEqual(OWNERSHIP_MAP_HEIGHT)
      for (const other of packed.slice(0, index)) {
        expect(Math.hypot(circle.x - other.x, circle.y - other.y)).toBeGreaterThanOrEqual(circle.radius + other.radius)
      }
    }
  })
})
