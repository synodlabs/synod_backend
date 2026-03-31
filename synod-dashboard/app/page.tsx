"use client"

import { useEffect } from "react"
import { useRouter } from "next/navigation"

export default function RootPage() {
  const router = useRouter()

  useEffect(() => {
    const token = localStorage.getItem("synod_token")
    if (token) {
      router.push("/dashboard")
    } else {
      router.push("/login")
    }
  }, [router])

  return (
    <div className="min-h-screen bg-synod-bg flex items-center justify-center">
      <div className="w-8 h-8 border-4 border-synod-accent/20 border-t-synod-accent rounded-full animate-spin" />
    </div>
  )
}
