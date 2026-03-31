"use client"

import { useState } from "react"
import { Cpu, Key, Terminal, Plus } from "lucide-react"
import { Button } from "@/components/ui/button"

interface AgentManagerProps {
  treasuryId: string;
  token: string | null;
}

export function AgentManager({ treasuryId, token }: AgentManagerProps) {
  const [loading, setLoading] = useState(false)
  const [newAgent, setNewAgent] = useState<{ id: string, key: string } | null>(null)

  const createAgent = async () => {
    if (!token) return
    setLoading(true)
    try {
      const res = await fetch("/v1/agents", {
        method: "POST",
        headers: { 
          "Content-Type": "application/json",
          "Authorization": `Bearer ${token}` 
        },
        body: JSON.stringify({ treasury_id: treasuryId, name: "New Strategy Agent" }),
      })
      const data = await res.json()
      setNewAgent({ id: data.agent_id, key: data.api_key })
    } catch (err) {
      console.error(err)
    } finally {
      setLoading(false)
    }
  }

  return (
    <section className="glass-card p-8 bg-black/20">
      <div className="flex items-center justify-between mb-8">
        <div className="flex items-center gap-3">
          <div className="text-synod-accent">
             <Cpu size={20} />
          </div>
          <h2 className="text-sm font-black text-white uppercase tracking-wider text-muted-foreground">Agent Swarm</h2>
        </div>
        <Button 
          onClick={createAgent}
          disabled={loading}
          variant="outline"
          size="sm"
          className="rounded-xl border-synod-accent/20 text-synod-accent hover:bg-synod-accent/10"
        >
          {loading ? "PROVISIONING..." : "ADD SLOT"}
        </Button>
      </div>

      {newAgent && (
        <div className="mb-6 p-4 bg-synod-accent/10 border border-synod-accent/30 rounded-xl relative overflow-hidden">
          <div className="absolute top-0 right-0 p-1 bg-synod-accent text-black text-[8px] font-black px-2 uppercase">SECRET_KEY</div>
          <p className="text-[10px] text-synod-accent font-black uppercase mb-2">Save it now! (One-time view)</p>
          <div className="flex items-center gap-3 bg-black/60 p-3 rounded-lg font-mono text-xs break-all border border-synod-accent/10">
            <Key size={14} className="text-synod-accent flex-shrink-0" />
            {newAgent.key}
          </div>
        </div>
      )}

      <div className="space-y-3">
        <div className="flex items-center justify-between p-4 bg-black/40 rounded-xl border border-white/5 group hover:border-synod-accent/20 transition-colors">
          <div className="flex items-center gap-4">
            <Terminal size={18} className="text-muted-foreground group-hover:text-synod-accent transition-colors" />
            <div>
              <div className="text-xs font-black text-white uppercase">PRIMARY_STRAT-NODE</div>
              <div className="text-[9px] text-muted-foreground font-mono tracking-tighter opacity-40">NODE_STATUS: DISCONNECTED</div>
            </div>
          </div>
          <div className="text-[8px] bg-red-500/20 text-red-400 px-2 py-1 rounded-md font-black uppercase tracking-widest border border-red-500/30">Offline</div>
        </div>
      </div>
    </section>
  )
}
