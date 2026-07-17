import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import App from './App'
import { ApiProvider } from './lib/api-context'
import { WalletProvider } from './lib/wallet-context'
import './styles.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <ApiProvider>
        <WalletProvider>
          <App />
        </WalletProvider>
      </ApiProvider>
    </BrowserRouter>
  </StrictMode>,
)
