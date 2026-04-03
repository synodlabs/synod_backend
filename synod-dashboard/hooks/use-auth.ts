"use client"

import { useState, useEffect, useCallback, useMemo } from "react"
import { useRouter } from "next/navigation"

export function useAuth() {
  const [token, setToken] = useState<string | null>(null)
  const router = useRouter()

  useEffect(() => {
    const storedToken = localStorage.getItem("synod_token")
    if (!storedToken) {
      router.push("/login")
    } else {
      setToken(storedToken)
    }
  }, [router])

  const logout = useCallback(() => {
    localStorage.removeItem("synod_token")
    router.push("/login")
  }, [router])

  const user = useMemo(() => ({ 
    name: "Ade Okonkwo", 
    avatar: "AO" 
  }), [])

  return { 
    token, 
    logout, 
    user 
  }
}
