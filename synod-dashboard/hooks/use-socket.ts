"use client"

import { useEffect, useRef, useState } from "react"

interface EventEnvelope {
  event_type: string
  payload: Record<string, any>
}

export function useSocket(token: string | null) {
  const [events, setEvents] = useState<EventEnvelope[]>([])
  const [lastState, setLastState] = useState<any>(null)
  const ws = useRef<WebSocket | null>(null)

  useEffect(() => {
    if (!token) return

    // Connect directly to the backend port (8080) for local development
    // since Next.js rewrites don't support WebSocket proxying out of the box.
    const wsUrl = `ws://localhost:8080/v1/dashboard/ws`
    
    ws.current = new WebSocket(wsUrl)

    ws.current.onmessage = (event) => {
      try {
        const envelope: EventEnvelope = JSON.parse(event.data)
        
        // Handle unified event format: { event_type, payload }
        if (envelope.event_type) {
          setEvents((prev) => [envelope, ...prev].slice(0, 50))

          // Handle specific events
          switch (envelope.event_type) {
            case "WALLET_BALANCE_UPDATE":
              setLastState((prev: any) => ({
                ...prev,
                balance_update: envelope.payload,
              }))
              break
            case "PERMIT_ISSUED":
            case "PERMIT_CONSUMED":
            case "PERMIT_EXPIRED":
              setLastState((prev: any) => ({
                ...prev,
                permit_event: envelope.payload,
              }))
              break
            case "TREASURY_HALTED":
            case "TREASURY_RESUMED":
              setLastState((prev: any) => ({
                ...prev,
                health_event: envelope.payload,
              }))
              break
            case "AGENT_STATUS_CHANGED":
            case "AGENT_CONNECTED":
            case "AGENT_ACTIVATED":
            case "AGENT_SUSPENDED":
              setLastState((prev: any) => ({
                ...prev,
                agent_event: envelope.payload,
              }))
              break
          }
        }
      } catch (err) {
        console.error("WS Parse Error:", err)
      }
    }

    ws.current.onerror = (err) => {
      console.error("WS Error:", err)
    }

    return () => {
      ws.current?.close()
    }
  }, [token])

  return { events, lastState }
}
