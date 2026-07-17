import { createContext, useContext, useMemo, useState, type ReactNode } from 'react'
import type { WalletSession } from '../types'

interface WalletContextValue {
  wallet?: WalletSession
  backupConfirmed: boolean
  setWallet: (wallet?: WalletSession) => void
  confirmBackup: () => void
  updateDisplayName: (name: string) => void
  lock: () => void
}

const WalletContext = createContext<WalletContextValue | undefined>(undefined)

export function WalletProvider({ children }: { children: ReactNode }) {
  const [wallet, setWalletState] = useState<WalletSession>()
  const [backupConfirmed, setBackupConfirmed] = useState(false)

  const value = useMemo<WalletContextValue>(() => ({
    wallet,
    backupConfirmed,
    setWallet: (next) => {
      setWalletState(next)
      setBackupConfirmed(false)
    },
    confirmBackup: () => setBackupConfirmed(true),
    updateDisplayName: (displayName) => setWalletState((current) => current ? { ...current, displayName } : current),
    lock: () => {
      setWalletState(undefined)
      setBackupConfirmed(false)
    },
  }), [wallet, backupConfirmed])

  return <WalletContext.Provider value={value}>{children}</WalletContext.Provider>
}

export function useWallet(): WalletContextValue {
  const context = useContext(WalletContext)
  if (!context) throw new Error('useWallet must be used within WalletProvider')
  return context
}
