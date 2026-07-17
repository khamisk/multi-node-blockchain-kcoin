import { describe, expect, it } from 'vitest'
import vectors from '../../../crates/kcoin-protocol/test-vectors/wallet.json'
import { decodeBech32m, encodeBech32m } from '../lib/bech32'
import { encodeBase64 } from '../lib/format'
import { canonicalTransactionBytes, publicKeyToAddress, signedTransactionId } from '../lib/wallet'

function fromHex(value: string): Uint8Array {
  return Uint8Array.from(value.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16))
}

function toHex(value: Uint8Array): string {
  return [...value].map((byte) => byte.toString(16).padStart(2, '0')).join('')
}

function arrayBuffer(value: Uint8Array): ArrayBuffer {
  return Uint8Array.from(value).buffer
}

const sender = fromHex(vectors.senderPublicKeyHex)
const common = {
  protocol_version: vectors.protocolVersion,
  chain_id: vectors.chainId,
  sender_public_key: encodeBase64(sender),
}

describe('Rust/browser protocol compatibility', () => {
  it('derives the Rust golden-vector address', async () => {
    expect(await publicKeyToAddress(sender)).toBe(vectors.senderAddress)
    expect(encodeBech32m('kcoin', decodeBech32m(vectors.senderAddress))).toBe(vectors.senderAddress)
  })

  it('matches transfer signing bytes byte-for-byte', () => {
    const transaction = {
      ...common,
      nonce: vectors.transfer.nonce,
      expiry_height: vectors.transfer.expiryHeight,
      action: { type: 'transfer', recipient: vectors.recipientAddress, amount_atoms: vectors.transfer.amountAtoms },
    } as const
    const bytes = canonicalTransactionBytes(transaction)
    expect(toHex(bytes)).toBe(vectors.transfer.signingBytesHex)
  })

  it('matches the Rust Ed25519 signature and signed transaction ID', async () => {
    const transaction = {
      ...common,
      nonce: vectors.transfer.nonce,
      expiry_height: vectors.transfer.expiryHeight,
      action: { type: 'transfer' as const, recipient: vectors.recipientAddress, amount_atoms: vectors.transfer.amountAtoms },
    }
    const privateKey = await crypto.subtle.importKey(
      'pkcs8',
      arrayBuffer(fromHex(vectors.senderPrivateKeyPkcs8Hex)),
      { name: 'Ed25519' },
      false,
      ['sign'],
    )
    const signature = new Uint8Array(await crypto.subtle.sign('Ed25519', privateKey, arrayBuffer(canonicalTransactionBytes(transaction))))
    expect(toHex(signature)).toBe(vectors.transfer.signatureHex)
    expect(signedTransactionId({ ...transaction, signature: btoa(String.fromCharCode(...signature)) })).toBe(vectors.transfer.transactionId)
  })

  it('matches reward-claim signing bytes byte-for-byte', () => {
    const bytes = canonicalTransactionBytes({
      ...common,
      nonce: vectors.claimReward.nonce,
      expiry_height: vectors.claimReward.expiryHeight,
      action: { type: 'claim_reward', challenge_id: vectors.claimReward.challengeId, answer: String(vectors.claimReward.answer) },
    })
    expect(toHex(bytes)).toBe(vectors.claimReward.signingBytesHex)
  })

  it('matches public-name signing bytes byte-for-byte', () => {
    const bytes = canonicalTransactionBytes({
      ...common,
      nonce: vectors.setDisplayName.nonce,
      expiry_height: vectors.setDisplayName.expiryHeight,
      action: { type: 'set_display_name', display_name: vectors.setDisplayName.displayName },
    })
    expect(toHex(bytes)).toBe(vectors.setDisplayName.signingBytesHex)
  })
})
