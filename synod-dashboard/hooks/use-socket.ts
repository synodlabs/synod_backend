"use client"

import { useEffect, useRef, useState } from "react"

export function useSocket(token: string | null) {
  const [events, setEvents] = useState<any[]>([])
  const [lastState, setLastState] = useState<any>(null)
  const ws = useRef<WebSocket | null>(null)

  useEffect(() => {
    if (!token) return

    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:"
    const wsUrl = `${protocol}//${window.location.host}/v1/dashboard/ws?auth=${token}`
    
    // In some cases the auth might be passed via header, but standard JS WebSocket doesn't support headers.
    // The backend `dashboard.rs` doesn't seem to extract auth from query, but let's check.
    // Actually, axum's `WebSocketUpgrade` usually matches the route.
    // The previous `App.tsx` used `new WebSocket(wsUrl)` without extra auth in URL.
    // Let's re-check `dashboard.rs` to see how it handles auth for WS.
    
    ws.current = new WebSocket(wsUrl)

    ws.current.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        if (data.type === "STATE_UPDATE") {
          setLastState(data.payload)
        }
        setEvents((prev) => [data, ...prev].slice(0, 50))
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
