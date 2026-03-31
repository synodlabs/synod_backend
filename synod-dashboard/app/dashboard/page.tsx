"use client"

import { useEffect, useState } from "react"
import { 
  Activity, 
  Shield, 
  Wallet, 
  Cpu, 
  LogOut, 
  Bell, 
  Zap, 
  TrendingUp, 
  Network,
  Plus,
  ArrowRight
} from "lucide-react"
import { useAuth } from "@/hooks/use-auth"
import { useSocket } from "@/hooks/use-socket"
import { Button } from "@/components/ui/button"
import { WalletConnect } from "@/components/dashboard/wallet-connect"
import { AgentManager } from "@/components/dashboard/agent-manager"

interface TreasuryState {
  treasury_id: string;
  name: string;
  health: 'HEALTHY' | 'HALTED' | 'PENDING_WALLET';
  current_aum_usd: number;
  peak_aum_usd: number;
  network: string;
  pools: Pool[];
}

interface Pool {
  pool_key: string;
  asset_code: string;
  target_pct: number;
  current_balance?: number;
}

export default function DashboardPage() {
  const { token, logout } = useAuth()
  const [state, setState] = useState<TreasuryState | null>(null)
  const { events, lastState } = useSocket(token)

  useEffect(() => {
    if (!token) return

    // 1. Fetch Treasuries
    fetch('/v1/dashboard', {
      headers: { 'Authorization': `Bearer ${token}` }
    })
    .then(res => res.json())
    .then(data => {
      if (data.length > 0) {
        const id = data[0].treasury_id;
        return fetch(`/v1/dashboard/${id}`, {
          headers: { 'Authorization': `Bearer ${token}` }
        });
      }
      throw new Error('No treasuries found');
    })
    .then(res => res?.json())
    .then(data => setState(data))
    .catch(err => console.error(err));
  }, [token]);

  // Update state from WebSocket
  useEffect(() => {
    if (lastState) {
      setState(prev => prev ? { ...prev, ...lastState } : null)
    }
  }, [lastState])

  if (!state) return (
    <div className="min-h-screen bg-synod-bg flex flex-col items-center justify-center text-synod-accent font-mono space-y-4">
      <div className="w-12 h-12 border-4 border-synod-accent/20 border-t-synod-accent rounded-full animate-spin" />
      <span className="animate-pulse tracking-widest uppercase text-xs font-bold">Initializing Synod_Core...</span>
    </div>
  )

  return (
    <div className="min-h-screen bg-synod-bg flex">
      {/* Sidebar Navigation */}
      <nav className="fixed left-0 top-0 h-full w-20 bg-black/40 border-r border-synod-border flex flex-col items-center py-10 gap-10 backdrop-blur-3xl z-50">
        <div className="p-3 bg-synod-accent/10 rounded-2xl border border-synod-accent/20 shadow-[0_0_15px_rgba(0,255,204,0.2)] animate-glow">
          <Shield className="text-synod-accent w-6 h-6" />
        </div>
        
        <div className="flex-1 flex flex-col gap-8">
          <NavItem icon={<Activity size={24} />} active />
          <NavItem icon={<Wallet size={24} />} />
          <NavItem icon={<Cpu size={24} />} />
          <NavItem icon={<TrendingUp size={24} />} />
        </div>

        <button onClick={logout} className="p-3 text-synod-error/60 hover:text-synod-error hover:bg-synod-error/10 rounded-2xl transition-all">
          <LogOut size={24} />
        </button>
      </nav>

      {/* Main Content Area */}
      <main className="flex-1 pl-20 min-h-screen">
        <div className="max-w-7xl mx-auto px-10 py-10">
          
          {/* Header Section */}
          <header className="flex justify-between items-start mb-14">
            <div>
              <div className="flex items-center gap-4 mb-2">
                <h1 className="text-4xl font-black tracking-tight text-white uppercase">{state.name}</h1>
                <span className={`text-[10px] font-black uppercase px-3 py-1 rounded-lg border tracking-widest ${state.health === 'HEALTHY' ? 'border-synod-accent/50 text-synod-accent bg-synod-accent/10' : 'border-synod-error/50 text-synod-error bg-synod-error/10'}`}>
                  {state.health}
                </span>
              </div>
              <p className="text-muted-foreground text-xs font-mono tracking-wider opacity-60">NODE_ID: {state.treasury_id}</p>
            </div>

            <div className="flex items-center gap-4">
              <Button variant="secondary" size="icon" className="rounded-2xl relative">
                <Bell size={20} />
                <span className="absolute top-2 right-2 w-2 h-2 bg-synod-error rounded-full" />
              </Button>
              <div className="glass-card px-5 py-3 flex items-center gap-3 border shadow-lg bg-black/60">
                <div className="w-2 h-2 bg-synod-accent rounded-full animate-pulse shadow-[0_0_10px_rgba(0,255,204,1)]" />
                <span className="text-[10px] font-black text-synod-accent uppercase tracking-[0.2em]">Live Sync Active</span>
              </div>
            </div>
          </header>

          {/* Core Metrics Grid */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-8 mb-14">
            <MetricCard 
              label="Treasury Liquidity" 
              value={`$${state.current_aum_usd.toLocaleString()}`} 
              subtext={`PEAK_AUM: $${state.peak_aum_usd.toLocaleString()}`}
              trend="+2.4%"
            />
            <MetricCard 
              label="Active Permits" 
              value="14" 
              subtext="POLICY GUARD: ACTIVE"
              accent="synod-accent"
            />
            <MetricCard 
              label="Stellar Network" 
              value={state.network.toUpperCase()} 
              subtext="CONNECTED TO HORIZON_RPC"
              icon={<Network size={20} className="text-muted-foreground"/>}
            />
          </div>

          <div className="grid grid-cols-1 lg:grid-cols-3 gap-10">
            {/* Component Management Area */}
            <div className="lg:col-span-2 space-y-12">
              
              {/* Pool Allocation */}
              <section>
                <div className="flex items-center justify-between mb-8">
                  <h2 className="text-xl font-black text-white flex items-center gap-3 uppercase tracking-wider">
                    <Activity className="text-synod-accent" size={20} />
                    Liquidity Pools
                  </h2>
                  <Button variant="outline" size="sm" className="rounded-xl border-white/10 hover:border-synod-accent/30">
                    REBALANCE ENTROPY
                  </Button>
                </div>
                
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-5">
                  {state.pools.map(pool => (
                    <div key={pool.pool_key} className="glass-card p-6 flex justify-between items-center group hover:bg-white/10">
                      <div>
                        <div className="text-xl font-black text-white group-hover:text-synod-accent transition-colors">{pool.asset_code}</div>
                        <div className="text-[10px] text-muted-foreground font-mono italic opacity-40">{pool.pool_key}</div>
                      </div>
                      <div className="text-right">
                        <div className="text-2xl font-black text-synod-accent">{pool.target_pct}%</div>
                        <div className="text-[10px] text-muted-foreground font-black uppercase tracking-widest">Weight</div>
                      </div>
                    </div>
                  ))}
                </div>
              </section>

              {/* Event Feed */}
              <section>
                <div className="flex items-center justify-between mb-8">
                  <h2 className="text-xl font-black text-white flex items-center gap-3 uppercase tracking-wider">
                    <Zap className="text-synod-accent" size={20} />
                    Coordination Signals
                  </h2>
                  <span className="text-[10px] font-black text-muted-foreground uppercase opacity-40">T-MINUS 0s</span>
                </div>
                
                <div className="glass-card overflow-hidden bg-black/40">
                  <div className="max-h-[400px] overflow-y-auto custom-scrollbar">
                    {events.length === 0 ? (
                      <div className="p-20 text-center text-muted-foreground italic text-sm animate-pulse">Waiting for incoming coordination signals...</div>
                    ) : (
                      events.map((ev, i) => (
                        <div key={i} className="px-8 py-5 border-b border-white/5 last:border-0 flex items-center justify-between hover:bg-synod-accent/5 transition-colors group">
                          <div className="flex items-center gap-6">
                            <div className="w-1.5 h-1.5 bg-synod-accent rounded-full shadow-[0_0_8px_rgba(0,255,204,1)]" />
                            <div>
                              <div className="text-sm font-black text-white group-hover:text-synod-accent transition-colors tracking-tight uppercase">{ev.type}</div>
                              <div className="text-[10px] text-muted-foreground font-mono mt-1 opacity-60">
                                {JSON.stringify(ev.payload).substring(0, 100)}...
                              </div>
                            </div>
                          </div>
                          <div className="text-[10px] font-black font-mono text-muted-foreground opacity-40 group-hover:opacity-100">{new Date().toLocaleTimeString()}</div>
                        </div>
                      ))
                    )}
                  </div>
                </div>
              </section>
            </div>

            {/* Side Operations Panel */}
            <div className="space-y-8">
              <WalletConnect 
                treasuryId={state.treasury_id} 
                token={token} 
                onSuccess={() => console.log('Wallet aligned')} 
              />
              <AgentManager 
                treasuryId={state.treasury_id} 
                token={token} 
              />
            </div>
          </div>
        </div>
      </main>
    </div>
  )
}

function NavItem({ icon, active = false }: { icon: React.ReactNode, active?: boolean }) {
  return (
    <div className={`p-4 rounded-2xl transition-all cursor-pointer group ${active ? 'bg-synod-accent/20 text-synod-accent shadow-[0_0_15px_rgba(0,255,204,0.1)]' : 'text-muted-foreground hover:bg-white/5 hover:text-white'}`}>
      {icon}
    </div>
  )
}

function MetricCard({ label, value, subtext, trend, accent, icon }: any) {
  return (
    <div className={`glass-card p-8 group overflow-hidden relative ${accent ? 'border-synod-accent/30' : ''}`}>
      {accent && <div className="absolute top-0 right-0 p-2 bg-synod-accent text-black text-[8px] font-black uppercase px-3 rounded-bl-xl tracking-tighter">Verified</div>}
      <div className="flex justify-between items-start mb-4">
        <h3 className="text-[10px] font-black uppercase tracking-[0.2em] text-muted-foreground">{label}</h3>
        {icon}
      </div>
      <div className="flex items-end gap-3">
        <div className="text-4xl font-black text-white group-hover:text-synod-accent transition-colors leading-none">{value}</div>
        {trend && <div className="text-[10px] font-black text-synod-accent mb-1">{trend}</div>}
      </div>
      <div className="mt-6 pt-4 border-t border-white/5 text-[10px] font-bold text-muted-foreground uppercase tracking-widest opacity-40">
        {subtext}
      </div>
    </div>
  )
}

function SidePanelCard({ title, icon, children }: any) {
  return (
    <section className="glass-card p-8 bg-black/20">
      <div className="flex items-center gap-3 mb-8">
        <div className="text-synod-accent">{icon}</div>
        <h2 className="text-sm font-black text-white uppercase tracking-wider">{title}</h2>
      </div>
      {children}
    </section>
  )
}
