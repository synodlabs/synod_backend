"use client"

import { useState } from "react"
import { Wallet, Plus, ChevronRight } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"

interface WalletConnectProps {
  treasuryId: string;
  token: string | null;
  onSuccess?: () => void;
}

export function WalletConnect({ treasuryId, token, onSuccess }: WalletConnectProps) {
  const [address, setAddress] = useState("")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState("")
  const [nonce, setNonce] = useState<string | null>(null)

  const handleRegister = async () => {
    if (!token) return
    setLoading(true)
    setError("")
    try {
      // 1. Get Nonce
      const nRes = await fetch("/v1/wallets/nonce", {
        headers: { "Authorization": `Bearer ${token}` }
      })
      const nData = await nRes.json()
      setNonce(nData.nonce)

      // 2. Register Wallet
      const res = await fetch(`/v1/treasuries/${treasuryId}/wallets`, {
        method: "POST",
        headers: { 
          "Content-Type": "application/json",
          "Authorization": `Bearer ${token}` 
        },
        body: JSON.stringify({ wallet_address: address, label: "Main Savings" }),
      })

      if (!res.ok) throw new Error("Failed to register wallet")
      
      setNonce(null)
      onSuccess?.()
    } catch (err: any) {
      setError(err.message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <section className="glass-card p-8 bg-black/20">
      <div className="flex items-center gap-3 mb-8">
        <div className="text-synod-accent">
          <Wallet size={20} />
        </div>
        <h2 className="text-sm font-black text-white uppercase tracking-wider text-muted-foreground mr-1">Wallet Consensus</h2>
      </div>

      <div className="space-y-6">
        <div className="space-y-2">
          <label className="text-[10px] font-black text-muted-foreground uppercase tracking-widest ml-1">Stellar G_Address</label>
          <Input
            placeholder="GA..."
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            className="h-10 text-xs font-mono"
          />
        </div>

        {error && <div className="text-[10px] text-synod-error font-bold uppercase text-center">{error}</div>}

        <Button
          onClick={handleRegister}
          disabled={loading || !address}
          variant={nonce ? "outline" : "primary"}
          className="w-full h-12 text-xs"
        >
          {loading ? "ESTABLISHING..." : nonce ? "SIGN NONCE" : "INITIATE CONSENSUS"}
        </Button>

        {nonce && (
          <div className="p-4 bg-synod-accent/5 border border-synod-accent/20 rounded-xl relative overflow-hidden">
            <div className="absolute top-0 right-0 p-1 bg-synod-accent text-black text-[8px] font-black px-2 uppercase">Pending</div>
            <p className="text-[10px] text-synod-accent font-black uppercase mb-2">Signature Required</p>
            <div className="text-[10px] font-mono text-white/60 break-all bg-black/40 p-2 rounded">
              {nonce}
            </div>
          </div>
        )}
      </div>
    </section>
  )
}
