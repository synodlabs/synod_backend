"use client"

import { useState, useEffect, useCallback, useMemo } from "react"
import { useRouter } from "next/navigation"

export function useAuth() {
  const [token, setToken] = useState<string | null>(null)
  const [userId, setUserId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const router = useRouter()

  useEffect(() => {
    // Check session via httpOnly cookie — call /me endpoint
    async function checkSession() {
      try {
        const res = await fetch("/v1/auth/me", {
          credentials: "include",
        })
        if (res.ok) {
          const data = await res.json()
          setUserId(data.user_id)
          setToken("cookie-auth") // Marker — actual token is in httpOnly cookie
        } else {
          router.push("/login")
        }
      } catch {
        router.push("/login")
      } finally {
        setLoading(false)
      }
    }
    checkSession()
  }, [router])

  const logout = useCallback(async () => {
    await fetch("/v1/auth/logout", {
      method: "POST",
      credentials: "include",
    })
    setToken(null)
    setUserId(null)
    router.push("/login")
  }, [router])

  const user = useMemo(() => ({ 
    name: "Ade Okonkwo", 
    avatar: "AO" 
  }), [])

  return { 
    token, 
    userId,
    loading,
    logout, 
    user 
  }
}
