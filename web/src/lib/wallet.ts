import { blake3 } from '@noble/hashes/blake3.js'
import { decodeBech32m, encodeBech32m } from './bech32'
import { decodeBase64, encodeBase64 } from './format'
import type { ExplorerTransaction, TransactionSubmission, WalletSession } from '../types'

interface WalletBackup {
  format: 'kcoin-pkcs8-v1'
  algorithm: 'Ed25519'
  created_at: string
  public_key_spki: string
  private_key_pkcs8: string
}

const encoder = new TextEncoder()
const TRANSACTION_SIGNATURE_PREFIX = encoder.encode('KCOIN_TX_V1\0')
const BACKUP_KEY_PROOF_PREFIX = encoder.encode('KCOIN_WALLET_BACKUP_KEY_PROOF_V1\0')

function arrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return Uint8Array.from(bytes).buffer
}

function concat(parts: Uint8Array[]): Uint8Array {
  const size = parts.reduce((total, part) => total + part.length, 0)
  const result = new Uint8Array(size)
  let offset = 0
  for (const part of parts) {
    result.set(part, offset)
    offset += part.length
  }
  return result
}

function u16(value: number): Uint8Array {
  const bytes = new Uint8Array(2)
  new DataView(bytes.buffer).setUint16(0, value, true)
  return bytes
}

function u32(value: number): Uint8Array {
  const bytes = new Uint8Array(4)
  new DataView(bytes.buffer).setUint32(0, value, true)
  return bytes
}

function u64(value: string): Uint8Array {
  const bytes = new Uint8Array(8)
  new DataView(bytes.buffer).setBigUint64(0, BigInt(value), true)
  return bytes
}

function u8(value: number): Uint8Array {
  return new Uint8Array([value])
}

function string(value: string): Uint8Array {
  const bytes = encoder.encode(value)
  return concat([u32(bytes.length), bytes])
}

/**
 * The protocol boundary for the browser. It mirrors the v1 Rust Borsh layout
 * and can be replaced by a generated WASM adapter later.
 */
export function canonicalTransactionBytes(transaction: Omit<TransactionSubmission, 'signature'>): Uint8Array {
  let action: Uint8Array
  if (transaction.action.type === 'transfer') {
    const recipient = decodeBech32m(transaction.action.recipient)
    if (recipient.length !== 20) throw new Error('Recipient must be a canonical KCoin address.')
    action = concat([u8(0), recipient, u64(transaction.action.amount_atoms)])
  } else if (transaction.action.type === 'claim_reward') {
    action = concat([u8(1), u64(transaction.action.challenge_id), u16(Number(transaction.action.answer))])
  } else {
    const name = transaction.action.display_name
    action = concat([u8(2), u8(name === null ? 0 : 1), ...(name === null ? [] : [string(name)])])
  }

  return concat([
    TRANSACTION_SIGNATURE_PREFIX,
    u16(transaction.protocol_version),
    string(transaction.chain_id),
    decodeBase64(transaction.sender_public_key),
    u64(transaction.nonce),
    u64(transaction.expiry_height),
    action,
  ])
}

/** Transaction ID over the canonical signed Borsh representation. */
export function signedTransactionId(transaction: TransactionSubmission): string {
  const unsigned = canonicalTransactionBytes(transaction).slice(TRANSACTION_SIGNATURE_PREFIX.length)
  const canonicalSigned = concat([unsigned, decodeBase64(transaction.signature)])
  const digest = blake3(canonicalSigned, { context: encoder.encode('kcoin.dev/v1/transaction-id') })
  return [...digest].map((byte) => byte.toString(16).padStart(2, '0')).join('')
}

/** True only when the exact submitted transaction appears in finalized history. */
export function hasFinalizedTransaction(
  transactionId: string,
  canonicalTransactions: readonly ExplorerTransaction[],
): boolean {
  return canonicalTransactions.some(
    (transaction) => transaction.id === transactionId && transaction.status === 'finalized',
  )
}

export async function publicKeyToAddress(publicKeyBytes: Uint8Array): Promise<string> {
  const digest = blake3(publicKeyBytes, { context: encoder.encode('kcoin.dev/v1/address') })
  return encodeBech32m('kcoin', digest.slice(0, 20))
}

async function createSession(
  privateKey: CryptoKey,
  publicKeyCrypto: CryptoKey,
  displayName = '',
): Promise<WalletSession> {
  const raw = new Uint8Array(await crypto.subtle.exportKey('raw', publicKeyCrypto))
  return {
    address: await publicKeyToAddress(raw),
    publicKey: encodeBase64(raw),
    publicKeyBytes: raw,
    privateKey,
    publicKeyCrypto,
    displayName,
  }
}

export function supportsEd25519(): boolean {
  return Boolean(globalThis.crypto?.subtle)
}

export async function generateWallet(displayName = ''): Promise<WalletSession> {
  const keys = await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify']) as CryptoKeyPair
  return createSession(keys.privateKey, keys.publicKey, displayName)
}

export async function exportWallet(session: WalletSession): Promise<Blob> {
  const [privateKey, publicKey] = await Promise.all([
    crypto.subtle.exportKey('pkcs8', session.privateKey),
    crypto.subtle.exportKey('spki', session.publicKeyCrypto),
  ])
  const backup: WalletBackup = {
    format: 'kcoin-pkcs8-v1',
    algorithm: 'Ed25519',
    created_at: new Date().toISOString(),
    public_key_spki: encodeBase64(new Uint8Array(publicKey)),
    private_key_pkcs8: encodeBase64(new Uint8Array(privateKey)),
  }
  return new Blob([JSON.stringify(backup, null, 2)], { type: 'application/json' })
}

export async function importWallet(file: File, displayName = ''): Promise<WalletSession> {
  let backup: WalletBackup
  try {
    backup = JSON.parse(await file.text()) as WalletBackup
  } catch {
    throw new Error('This is not a valid KCoin wallet backup.')
  }
  if (backup.format !== 'kcoin-pkcs8-v1' || backup.algorithm !== 'Ed25519') {
    throw new Error('Unsupported wallet backup format.')
  }
  try {
    const [privateKey, publicKey] = await Promise.all([
      crypto.subtle.importKey('pkcs8', arrayBuffer(decodeBase64(backup.private_key_pkcs8)), { name: 'Ed25519' }, true, ['sign']),
      crypto.subtle.importKey('spki', arrayBuffer(decodeBase64(backup.public_key_spki)), { name: 'Ed25519' }, true, ['verify']),
    ])
    // PKCS#8 and SPKI are independently valid containers, so successful imports
    // alone do not prove that they describe the same key pair. Sign a fresh,
    // domain-separated proof and verify it with the backup's public key before
    // creating a wallet session.
    const proof = concat([BACKUP_KEY_PROOF_PREFIX, crypto.getRandomValues(new Uint8Array(32))])
    const signature = await crypto.subtle.sign('Ed25519', privateKey, arrayBuffer(proof))
    const matches = await crypto.subtle.verify('Ed25519', publicKey, signature, arrayBuffer(proof))
    if (!matches) throw new Error('The private and public keys in this backup do not match.')
    return createSession(privateKey, publicKey, displayName)
  } catch (reason) {
    if (reason instanceof Error && reason.message.includes('do not match')) throw reason
    throw new Error('The wallet key could not be imported.')
  }
}

export async function signTransaction(
  session: WalletSession,
  transaction: Omit<TransactionSubmission, 'signature'>,
): Promise<TransactionSubmission> {
  const signature = await crypto.subtle.sign('Ed25519', session.privateKey, arrayBuffer(canonicalTransactionBytes(transaction)))
  return {
    ...transaction,
    signature: encodeBase64(new Uint8Array(signature)),
  }
}

export function downloadWalletBackup(blob: Blob, address: string): void {
  const link = document.createElement('a')
  link.href = URL.createObjectURL(blob)
  link.download = `kcoin-${address.slice(0, 13)}-pkcs8.json`
  link.click()
  URL.revokeObjectURL(link.href)
}
