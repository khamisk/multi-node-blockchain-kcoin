import { describe, expect, it } from 'vitest'
import { resolveSearchQuery } from '../lib/search'

const hash = 'ab'.repeat(32)

describe('global explorer search', () => {
  it('routes unambiguous heights and addresses directly', () => {
    expect(resolveSearchQuery('1642')).toEqual({ kind: 'route', route: '/blocks/1642' })
    expect(resolveSearchQuery('kcoin1qqqqqq')).toEqual({ kind: 'route', route: '/address/kcoin1qqqqqq' })
  })

  it('asks the user to disambiguate a bare 64-character hash', () => {
    expect(resolveSearchQuery(hash)).toEqual({ kind: 'ambiguous-hash', hash })
  })

  it('supports explicit block and transaction hash prefixes', () => {
    expect(resolveSearchQuery(`block:${hash}`)).toEqual({ kind: 'route', route: `/blocks/${hash}` })
    expect(resolveSearchQuery(`tx ${hash}`)).toEqual({ kind: 'route', route: `/transactions/${hash}` })
  })

  it('does not silently guess for malformed input', () => {
    expect(resolveSearchQuery('maybe-a-hash').kind).toBe('invalid')
  })
})
