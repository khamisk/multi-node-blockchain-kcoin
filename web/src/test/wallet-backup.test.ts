import { describe, expect, it } from 'vitest'
import { encodeBase64 } from '../lib/format'
import { generateWallet, importWallet } from '../lib/wallet'
import type { WalletSession } from '../types'

async function backupFile(privateOwner: WalletSession, publicOwner = privateOwner): Promise<File> {
  const [privateKey, publicKey] = await Promise.all([
    crypto.subtle.exportKey('pkcs8', privateOwner.privateKey),
    crypto.subtle.exportKey('spki', publicOwner.publicKeyCrypto),
  ])
  const json = JSON.stringify({
    format: 'kcoin-pkcs8-v1',
    algorithm: 'Ed25519',
    created_at: new Date(0).toISOString(),
    private_key_pkcs8: encodeBase64(new Uint8Array(privateKey)),
    public_key_spki: encodeBase64(new Uint8Array(publicKey)),
  })
  return { text: async () => json } as File
}

describe('wallet backups', () => {
  it('restores a backup only after proving its key pair matches', async () => {
    const original = await generateWallet('Original')
    const restored = await importWallet(await backupFile(original), 'Restored')

    expect(restored.address).toBe(original.address)
    expect(restored.publicKey).toBe(original.publicKey)
    expect(restored.displayName).toBe('Restored')
  })

  it('rejects a backup whose PKCS#8 private key and SPKI public key differ', async () => {
    const privateOwner = await generateWallet()
    const differentPublicOwner = await generateWallet()

    await expect(importWallet(await backupFile(privateOwner, differentPublicOwner)))
      .rejects.toThrow('private and public keys in this backup do not match')
  })
})
