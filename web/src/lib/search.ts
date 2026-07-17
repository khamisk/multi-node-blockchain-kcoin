export type SearchResolution =
  | { kind: 'route'; route: string }
  | { kind: 'ambiguous-hash'; hash: string }
  | { kind: 'invalid'; message: string }

const HASH = /^[0-9a-f]{64}$/i
const ADDRESS = /^kcoin1[023456789acdefghjklmnpqrstuvwxyz]+$/i

function explicitHash(value: string, prefixes: RegExp, route: 'blocks' | 'transactions'): SearchResolution | undefined {
  const match = value.match(prefixes)
  if (!match) return undefined
  const hash = match[1]
  return HASH.test(hash)
    ? { kind: 'route', route: `/${route}/${hash.toLowerCase()}` }
    : { kind: 'invalid', message: `${route === 'blocks' ? 'Block' : 'Transaction'} hashes must be 64 hexadecimal characters.` }
}

/** A deterministic parser keeps global search useful without guessing hash type. */
export function resolveSearchQuery(query: string): SearchResolution {
  const value = query.trim()
  if (!value) return { kind: 'invalid', message: 'Enter a block height, hash, or wallet address.' }

  const block = explicitHash(value, /^(?:block|b)(?:\s+|:)(\S+)$/i, 'blocks')
  if (block) return block
  const transaction = explicitHash(value, /^(?:transaction|tx)(?:\s+|:)(\S+)$/i, 'transactions')
  if (transaction) return transaction

  if (ADDRESS.test(value)) return { kind: 'route', route: `/address/${value.toLowerCase()}` }
  if (/^\d+$/.test(value)) return { kind: 'route', route: `/blocks/${value}` }
  if (HASH.test(value)) return { kind: 'ambiguous-hash', hash: value.toLowerCase() }

  return { kind: 'invalid', message: 'Use a height, kcoin1 address, or 64-character hash. Prefix a hash with block: or tx: to go directly.' }
}
