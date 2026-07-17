import { Route, Routes } from 'react-router-dom'
import { AppShell } from './components/app-shell'
import { AddressPage } from './pages/address'
import { BlocksPage } from './pages/blocks'
import { EarnPage } from './pages/earn'
import { NotFoundPage } from './pages/not-found'
import { OverviewPage } from './pages/overview'
import { OwnershipPage } from './pages/ownership'
import { TransactionsPage } from './pages/transactions'
import { ValidatorsPage } from './pages/validators'
import { WalletPage } from './pages/wallet'

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<OverviewPage />} />
        <Route path="/earn" element={<EarnPage />} />
        <Route path="/blocks" element={<BlocksPage />} />
        <Route path="/blocks/:id" element={<BlocksPage />} />
        <Route path="/transactions" element={<TransactionsPage />} />
        <Route path="/transactions/:id" element={<TransactionsPage />} />
        <Route path="/address/:address" element={<AddressPage />} />
        <Route path="/ownership" element={<OwnershipPage />} />
        <Route path="/validators" element={<ValidatorsPage />} />
        <Route path="/wallet" element={<WalletPage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Routes>
    </AppShell>
  )
}
