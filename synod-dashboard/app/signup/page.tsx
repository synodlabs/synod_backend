"use client"

import { useState } from "react"
import { useRouter } from "next/navigation"
import { Shield, Lock, Mail, ArrowRight, UserPlus } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"

export default function SignupPage() {
  const [email, setEmail] = useState("")
  const [password, setPassword] = useState("")
  const [confirmPassword, setConfirmPassword] = useState("")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState("")
  const router = useRouter()

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (password !== confirmPassword) {
      setError("PASSWORDS_DO_NOT_MATCH")
      return
    }

    setLoading(true)
    setError("")

    try {
      const res = await fetch("/v1/auth/register", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      })

      if (!res.ok) {
        const data = await res.json()
        throw new Error(data.message || "Registration failed")
      }

      const data = await res.json()
      localStorage.setItem("synod_token", data.token)
      router.push("/dashboard")
    } catch (err: any) {
      setError(err.message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center p-4 relative overflow-hidden bg-[radial-gradient(circle_at_bottom_left,_var(--tw-gradient-stops))] from-synod-accent/10 via-synod-bg to-synod-bg">
      {/* Decorative Elements */}
      <div className="absolute top-[-10%] left-[-10%] w-[40%] h-[40%] bg-synod-accent/5 blur-[120px] rounded-full" />
      <div className="absolute bottom-[-10%] right-[-10%] w-[30%] h-[30%] bg-synod-error/5 blur-[100px] rounded-full" />

      <div className="w-full max-w-md z-10">
        <div className="text-center mb-10 space-y-2">
          <div className="inline-flex p-4 bg-synod-accent/10 rounded-2xl border border-synod-accent/20 mb-4 animate-glow">
            <UserPlus className="text-synod-accent w-10 h-10" />
          </div>
          <h1 className="text-4xl font-black tracking-tight text-white uppercase">
            Join<span className="text-synod-accent">_</span>Synod
          </h1>
          <p className="text-muted-foreground font-medium">Create your governance identity</p>
        </div>

        <div className="glass-card p-8 shadow-2xl relative">
          <div className="absolute top-0 left-0 w-full h-1 bg-gradient-to-r from-transparent via-synod-accent/50 to-transparent" />
          
          <form onSubmit={handleSubmit} className="space-y-6">
            <div className="space-y-2">
              <label className="text-xs font-black uppercase tracking-widest text-muted-foreground ml-1">Email Address</label>
              <Input
                type="email"
                placeholder="admin@synod.xyz"
                icon={<Mail size={18} />}
                required
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </div>

            <div className="space-y-2">
              <label className="text-xs font-black uppercase tracking-widest text-muted-foreground ml-1">Access Key</label>
              <Input
                type="password"
                placeholder="••••••••"
                icon={<Lock size={18} />}
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>

            <div className="space-y-2">
              <label className="text-xs font-black uppercase tracking-widest text-muted-foreground ml-1">Confirm Key</label>
              <Input
                type="password"
                placeholder="••••••••"
                icon={<Lock size={18} />}
                required
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
              />
            </div>

            {error && (
              <div className="bg-synod-error/10 border border-synod-error/30 text-synod-error text-xs font-bold p-3 rounded-xl text-center">
                {error.toUpperCase()}
              </div>
            )}

            <Button
              type="submit"
              disabled={loading}
              className="w-full h-14 group"
            >
              {loading ? (
                <span className="flex items-center gap-2">
                  <div className="w-4 h-4 border-2 border-black/30 border-t-black rounded-full animate-spin" />
                  PROVISIONING...
                </span>
              ) : (
                <span className="flex items-center gap-2">
                  CREATE IDENTITY
                  <ArrowRight size={18} className="group-hover:translate-x-1 transition-transform" />
                </span>
              )}
            </Button>
          </form>

          <div className="mt-8 pt-6 border-t border-synod-border text-center">
            <p className="text-xs text-muted-foreground">
              Already have a node? <button onClick={() => router.push("/login")} className="text-synod-accent font-bold hover:underline">RECONNECT</button>
            </p>
          </div>
        </div>
      </div>
    </div>
  )
}
