import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import type { ApiEvent, DataMode, ExplorerTransport } from '../types'
import { demoTransport } from './demo'
import { RestTransport } from './api'

interface ApiContextValue {
  transport: ExplorerTransport
  mode: DataMode
  ready: boolean
  statusRevision: number
  historyRevision: number
  refresh: () => void
}

const ApiContext = createContext<ApiContextValue | undefined>(undefined)
const liveTransport = new RestTransport()

export function ApiProvider({ children }: { children: ReactNode }) {
  const forcedDemo = import.meta.env.VITE_DEMO_MODE === 'always'
  const [transport, setTransport] = useState<ExplorerTransport>(forcedDemo ? demoTransport : liveTransport)
  const [mode, setMode] = useState<DataMode>(forcedDemo ? 'demo' : 'live')
  const [ready, setReady] = useState(forcedDemo)
  const [statusRevision, setStatusRevision] = useState(0)
  const [historyRevision, setHistoryRevision] = useState(0)
  const statusRefreshTimer = useRef<number | undefined>(undefined)
  const refresh = useCallback(() => {
    setStatusRevision((value) => value + 1)
    setHistoryRevision((value) => value + 1)
  }, [])
  const consumeEvent = useCallback((event: ApiEvent) => {
    if (statusRefreshTimer.current === undefined) {
      statusRefreshTimer.current = window.setTimeout(() => {
        statusRefreshTimer.current = undefined
        setStatusRevision((value) => value + 1)
      }, 400)
    }
    if (event.type === 'finalized_block' || event.type === 'transaction') {
      setHistoryRevision((value) => value + 1)
    }
  }, [])

  useEffect(() => () => {
    if (statusRefreshTimer.current !== undefined) window.clearTimeout(statusRefreshTimer.current)
  }, [])

  useEffect(() => {
    if (forcedDemo) return
    const controller = new AbortController()
    const timeout = window.setTimeout(() => controller.abort(), 1300)
    liveTransport.status(controller.signal)
      .then(() => {
        setTransport(liveTransport)
        setMode('live')
      })
      .catch(() => {
        if (import.meta.env.VITE_DEMO_MODE !== 'never') {
          setTransport(demoTransport)
          setMode('demo')
        }
      })
      .finally(() => {
        window.clearTimeout(timeout)
        setReady(true)
      })
    return () => {
      controller.abort()
      window.clearTimeout(timeout)
    }
  }, [forcedDemo])

  useEffect(() => {
    if (!ready) return
    return transport.subscribe(consumeEvent)
  }, [ready, transport, consumeEvent])

  const value = useMemo(() => ({ transport, mode, ready, statusRevision, historyRevision, refresh }), [transport, mode, ready, statusRevision, historyRevision, refresh])
  return <ApiContext.Provider value={value}>{children}</ApiContext.Provider>
}

export function useApi(): ApiContextValue {
  const context = useContext(ApiContext)
  if (!context) throw new Error('useApi must be used within ApiProvider')
  return context
}
