import { describe, expect, it } from 'vitest'
import { createDemoTransport, demoRewardForSupply, demoTransport, evaluateDemoChallenge } from '../lib/demo'
import type { Challenge, TransactionSubmission } from '../types'

function claimSubmission(challenge: Challenge): TransactionSubmission {
  const bytes = new Uint8Array(32).fill(7)
  return {
    protocol_version: 1,
    chain_id: 'kcoin-localnet-1',
    sender_public_key: btoa(String.fromCharCode(...bytes)),
    nonce: '0',
    expiry_height: '999999',
    action: {
      type: 'claim_reward',
      challenge_id: challenge.challenge_id,
      answer: evaluateDemoChallenge(challenge.expression).toString(),
    },
    signature: btoa(String.fromCharCode(...new Uint8Array(64))),
  }
}

describe('demo challenges', () => {
  it('evaluates every supported operation shown by the demo', () => {
    expect(evaluateDemoChallenge('7 × 6')).toBe(42)
    expect(evaluateDemoChallenge('9 − 4')).toBe(5)
    expect(evaluateDemoChallenge('3 + 8')).toBe(11)
  })

  it('fails closed for malformed challenge text', () => {
    expect(() => evaluateDemoChallenge('7 / 1')).toThrow('malformed')
  })

  it('mirrors all five issuance bands and the hard cap', () => {
    const kcoin = 1_000_000n
    expect(demoRewardForSupply(0n)).toBe(100n * kcoin)
    expect(demoRewardForSupply(20_000n * kcoin)).toBe(50n * kcoin)
    expect(demoRewardForSupply(40_000n * kcoin)).toBe(25n * kcoin)
    expect(demoRewardForSupply(60_000n * kcoin)).toBe(10n * kcoin)
    expect(demoRewardForSupply(80_000n * kcoin)).toBe(5n * kcoin)
    expect(demoRewardForSupply(100_000n * kcoin - 1n)).toBe(1n)
    expect(demoRewardForSupply(100_000n * kcoin)).toBe(0n)
  })

  it('seeds the visible demo in the 25 KCoin issuance band', async () => {
    await expect(demoTransport.challenge()).resolves.toMatchObject({ reward_atoms: '25000000' })
  })

  it('halts at two validators and certificates use only online signers', async () => {
    const halted = createDemoTransport()
    await halted.setValidatorOnline?.(0, false)
    await halted.setValidatorOnline?.(1, false)
    const before = await halted.status()
    await expect(halted.submit(claimSubmission(await halted.challenge()))).rejects.toMatchObject({ code: 'FINALITY_UNAVAILABLE' })
    await expect(halted.status()).resolves.toMatchObject({ height: before.height })

    const quorum = createDemoTransport()
    await quorum.setValidatorOnline?.(0, false)
    const start = await quorum.status()
    await quorum.submit(claimSubmission(await quorum.challenge()))
    const finalized = await quorum.block((BigInt(start.height) + 1n).toString())
    expect(finalized.certificate?.signatures.map((signature) => signature.validator)).toEqual([
      'validator-2',
      'validator-3',
      'validator-4',
    ])
  })
})
