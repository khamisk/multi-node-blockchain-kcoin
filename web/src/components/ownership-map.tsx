import { useMemo } from 'react'
import { formatKcoin, percentFromBps, shortHash } from '../lib/format'
import {
  OWNERSHIP_MAP_HEIGHT,
  OWNERSHIP_MAP_WIDTH,
  buildOwnershipVisualEntries,
  packOwnershipEntries,
} from '../lib/ownership'
import type { LeaderboardEntry } from '../types'

export function OwnershipMap({ entries, circulatingSupplyAtoms }: { entries: LeaderboardEntry[]; circulatingSupplyAtoms: string }) {
  const visualEntries = useMemo(
    () => buildOwnershipVisualEntries(entries, circulatingSupplyAtoms),
    [entries, circulatingSupplyAtoms],
  )
  const packed = useMemo(() => packOwnershipEntries(visualEntries), [visualEntries])
  const omitted = visualEntries.find((entry) => entry.aggregate)

  return (
    <div className="ownership-map">
      <svg viewBox={`0 0 ${OWNERSHIP_MAP_WIDTH} ${OWNERSHIP_MAP_HEIGHT}`} role="img" aria-labelledby="ownership-map-title ownership-map-description">
        <title id="ownership-map-title">KCoin ownership map</title>
        <desc id="ownership-map-description">The top 30 wallets are shown individually, with all remaining holders grouped in one labelled circle. Every circle area is exactly proportional to its finalized KCoin balance.</desc>
        {packed.map((entry, index) => (
          <g
            key={entry.key}
            className={`ownership-node ${entry.aggregate ? 'ownership-node--aggregate' : ''}`}
          >
            <circle cx={entry.x} cy={entry.y} r={entry.radius} fill={entry.aggregate ? '#e8e8e3' : '#ad6500'} fillOpacity={entry.aggregate ? 1 : Math.max(.42, .9 - index * .045)} />
            {entry.radius >= 25 && <text x={entry.x} y={entry.y - (entry.radius >= 43 ? 7 : 0)} textAnchor="middle" className="ownership-node__name">{entry.aggregate ? 'Other holders' : entry.entry?.display_name ?? `#${entry.entry?.rank}`}</text>}
            {entry.radius >= 38 && <text x={entry.x} y={entry.y + 15} textAnchor="middle" className="ownership-node__share">{percentFromBps(entry.share_bps)}</text>}
            <title>{entry.label ?? shortHash(entry.key)}: {formatKcoin(entry.balance_atoms)} KCoin</title>
          </g>
        ))}
      </svg>
      <div className="map-key">
        <span><i /> Circle area equals finalized balance</span>
        <span>Top {Math.min(30, entries.length)} wallets{omitted ? ' plus other holders' : ''}</span>
      </div>
    </div>
  )
}
