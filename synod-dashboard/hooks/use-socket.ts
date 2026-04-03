"use client"

import { useEffect, useRef, useState } from "react"

export function useSocket(token: string | null) {
  const [events, setEvents] = useState<any[]>([])
  const [lastState, setLastState] = useState<any>(null)
  const ws = useRef<WebSocket | null>(null)

  useEffect(() => {
    if (!token) return

    // Connect directly to the backend port (8080) for local development
    // since Next.js rewrites don't support WebSocket proxying out of the box.
    const wsUrl = `ws://localhost:8080/v1/dashboard/ws?auth=${token}`
    
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
