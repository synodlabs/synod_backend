"use client"

import { useEffect, useState } from "react"
import {
  TrendingUp,
  TrendingDown,
  ArrowUpRight,
  ArrowDownRight,
  AlertTriangle,
  RefreshCw,
  Plus,
  Shield
} from "lucide-react"
import { Horizon } from '@stellar/stellar-sdk'
import { useAuth } from "@/hooks/use-auth"
import { useSocket } from "@/hooks/use-socket"
import { Sidebar } from "@/components/dashboard/sidebar"
import { Topbar } from "@/components/dashboard/topbar"
import { WalletConnect } from "@/components/dashboard/wallet-connect"
import { WalletCard } from "@/components/dashboard/wallet-card"
import { AgentManager, type AgentSlot } from "@/components/dashboard/agent-manager"
import { PolicyManager } from "@/components/dashboard/policy-manager"
import { Button } from "@/components/ui/button"

interface Pool {
  pool_key: string;
  asset_code: string;
  target_pct: number;
  current_balance?: number;
}

interface Wallet {
  wallet_address: string;
  label: string | null;
  multisig_active: boolean;
  status: string;
}

interface TreasuryState {
  treasury_id: string;
  name: string;
  health: 'HEALTHY' | 'HALTED' | 'DEGRADED' | 'PENDING_WALLET';
  current_aum_usd: number;
  peak_aum_usd: number;
  network: string;
  pools: Pool[];
  wallets: Wallet[];
}

interface BalanceEntry {
  balance: string;
  asset_type: string;
  asset_code?: string;
}

interface KPICardProps {
  label: string;
  value: number | string;
  change: string;
  trend: 'up' | 'neutral';
  isLoading?: boolean;
  isCurrency?: boolean;
}

interface DashboardEvent {
  event_type?: string;
  type?: string;
  payload?: unknown;
  [key: string]: unknown;
}

export default function DashboardPage() {
  const { token, logout, user } = useAuth()
  const [state, setState] = useState<TreasuryState | null>(null)
  const [loading, setLoading] = useState(true)
  const [noTreasury, setNoTreasury] = useState(false)
  const [activeTab, setActiveTab] = useState<'overview' | 'wallets' | 'agents' | 'policy' | 'permits' | 'activity' | 'settings'>('overview')
  const [walletBalances, setWalletBalances] = useState<Record<string, number>>({})
  const [agentsData, setAgentsData] = useState<AgentSlot[]>([])
  const [aumLoaded, setAumLoaded] = useState(false)
  const [policyFocusAgentId, setPolicyFocusAgentId] = useState<string | null>(null)
  const { events, lastState } = useSocket(token)

  const totalAum = Object.values(walletBalances).reduce((a, b) => a + b, 0)
  const displayAum = totalAum > 0 ? totalAum : (state?.current_aum_usd || 0)

  const fetchAgents = async (treasuryId: string, authToken = token) => {
    if (!authToken) return

    try {
      const res = await fetch(`/v1/agents/${treasuryId}`, {
        headers: { 'Authorization': `Bearer ${authToken}` }
      })

      if (!res.ok) throw new Error("Failed to fetch agents")

      const data: AgentSlot[] = await res.json()
      setAgentsData(data)
    } catch (err) {
      console.error(err)
    }
  }

  const fetchData = async () => {
    if (!token) return
    setLoading(true)
    try {
      const res1 = await fetch('/v1/dashboard', {
        headers: { 'Authorization': `Bearer ${token}` }
      })
      if (!res1.ok) throw new Error("Failed to fetch dashboard list")
      const dashData = await res1.json()

      if (Array.isArray(dashData) && dashData.length > 0) {
        const id = dashData[0].treasury_id;
        const res2 = await fetch(`/v1/dashboard/${id}`, {
          headers: { 'Authorization': `Bearer ${token}` }
        })
        if (!res2.ok) throw new Error("Failed to fetch treasury state")
        const stateData = await res2.json()
        setState(stateData)
        setNoTreasury(false)
        await fetchAgents(id, token)

      } else {
        setNoTreasury(true)
      }
    } catch (err) {
      console.error(err)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    fetchData()
  }, [token]);

  useEffect(() => {
    if (lastState) {
      setState(prev => prev ? { ...prev, ...lastState } : null)
    }
    if (events[0] && ["AGENT_CONNECTED", "AGENT_ACTIVATED", "AgentStatusChanged", "AgentSuspended"].includes(events[0].event_type)) {
      if (state?.treasury_id && token) {
        fetchAgents(state.treasury_id, token).catch(err => console.error(err))
      }
    }
  }, [events, lastState, state?.treasury_id, token])

  const handleManageRules = (agentId: string) => {
    setPolicyFocusAgentId(agentId)
    setActiveTab('policy')
  }

  const handleOpenAgentSlot = (_agentId: string) => {
    setActiveTab('agents')
  }

  // Background balance fetcher for AUM loading state
  useEffect(() => {
    if (!state?.wallets) return;
    if (state.wallets.length === 0) {
      setAumLoaded(true);
      return;
    }

    let isMounted = true;
    const server = new Horizon.Server("https://horizon-testnet.stellar.org");

    async function fetchBalances() {
      try {
        const balances = { ...walletBalances };
        for (const w of state!.wallets) {
          try {
            const account = await server.loadAccount(w.wallet_address);
            const usdValues = account.balances.map((b: BalanceEntry) => {
              const amount = parseFloat(b.balance);
              return b.asset_type === "native" ? amount * 0.15 : (b.asset_code === "USDC" ? amount : 0);
            });
            balances[w.wallet_address] = usdValues.reduce((sum, val) => sum + val, 0);
          } catch (e) {
            // Ignore if account not found
          }
        }
        if (isMounted) {
          setWalletBalances(prev => ({ ...prev, ...balances }));
          setAumLoaded(true);
        }
      } catch (err) { }
    }

    fetchBalances();
    return () => { isMounted = false };
  }, [state?.wallets]);

  const triggerResync = async () => {
    if (!token || !state) return;
    try {
      await fetch(`/v1/treasuries/${state.treasury_id}/resync`, {
        method: 'POST',
        headers: { 'Authorization': `Bearer ${token}` }
      });
    } catch (e) {
      console.error(e);
    }
  }

  const handleProvision = async () => {
    if (!token) return;
    setLoading(true);
    try {
      const res = await fetch('/v1/treasuries', {
        method: 'POST',
        headers: {
          'Authorization': `Bearer ${token}`,
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({
          name: "Primary Treasury",
          network: "testnet"
        })
      });
      if (!res.ok) throw new Error("Failed to provision treasury");
      await fetchData();
    } catch (err) {
      console.error(err);
      alert("Failed to provision treasury. Please try again.");
    } finally {
      setLoading(false);
    }
  }

  if (loading) return (
    <div className="min-h-screen bg-synod-bg flex flex-col items-center justify-center text-white font-mono space-y-4">
      <div className="w-10 h-10 border-2 border-white/10 border-t-white rounded-full animate-spin" />
      <span className="tracking-[0.3em] uppercase text-[10px] font-bold opacity-50">Synchronizing_Core...</span>
    </div>
  )

  if (noTreasury || !state) return (
    <div className="min-h-screen bg-synod-bg flex items-center justify-center p-4">
      <div className="max-w-md w-full text-center space-y-12">
        <div className="inline-flex p-8 bg-white/5 rounded-full border border-white/10">
          <Shield className="text-white w-12 h-12" />
        </div>
        <div className="space-y-4">
          <h1 className="text-3xl font-black text-white uppercase tracking-tighter">System Initialization</h1>
          <p className="text-synod-muted text-sm">No active treasuries found. Provision a primary treasury to continue.</p>
        </div>
        <button
          onClick={handleProvision}
          className="w-full h-14 bg-white text-black font-bold uppercase tracking-widest text-xs hover:bg-zinc-200 transition-all"
        >
          Provision Primary Treasury
        </button>
      </div>
    </div>
  )

  return (
    <div className="min-h-screen bg-synod-bg flex flex-row overflow-hidden">
      <Sidebar
        activeTab={activeTab}
        onTabChange={setActiveTab}
        user={{ name: user?.name || "Ade Okonkwo", avatar: "AO" }}
        badges={{ wallets: state.wallets?.length || 0, agents: agentsData.length, permits: 5 }}
      />

      <div className="flex-1 flex flex-col h-screen overflow-hidden">
        <Topbar
          title={activeTab.charAt(0).toUpperCase() + activeTab.slice(1)}
          subtitle={activeTab === 'agents' ? `/ ${agentsData.length} slots` : `/ ${state.name || 'treasury-1'}`}
          health={state.health || 'HEALTHY'}
          onResync={triggerResync}
        />

        <main className="flex-1 overflow-y-auto p-8 custom-scrollbar">
          <div className="max-w-7xl mx-auto">
            {activeTab === 'overview' && (
              <div className="space-y-8">
                {/* KPI Grid */}
                <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                  <KPICard
                    label="Total AUM"
                    value={displayAum}
                    isLoading={!aumLoaded}
                    isCurrency
                    change="+3.2% today"
                    trend="up"
                  />
                  <KPICard
                    label="Active Permits"
                    value="5"
                    change="of 10 cap"
                    trend="neutral"
                  />
                  <KPICard
                    label="Agents Active"
                    value={`${agentsData.filter(a => a.status === 'ACTIVE').length} / ${agentsData.length}`}
                    change={`${agentsData.length === 0 ? '0' : agentsData.filter(a => a.status !== 'ACTIVE').length} inactive`}
                    trend="neutral"
                  />
                </div>

                <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
                  {/* Left Column: Agent Manager (Top Left) & Signal Feed */}
                  <div className="lg:col-span-2 space-y-6">
                    <AgentManager
                      treasuryId={state.treasury_id}
                      token={token}
                      agents={agentsData}
                      onAgentsChange={() => fetchAgents(state.treasury_id)}
                      isDashboardWidget={true}
                    />

                    <section className="bg-synod-card border border-synod-border rounded-md">
                      <div className="p-5 border-b border-synod-border">
                        <h2 className="text-sm font-bold text-white">Signal Feed</h2>
                      </div>
                      <div className="divide-y divide-synod-border">
                        {events.length === 0 ? (
                          <div className="p-12 text-center text-synod-muted text-xs font-mono">Awaiting coordination signals...</div>
                        ) : (
                          events.map((ev, i) => (
                            <EventRow key={i} event={ev} />
                          ))
                        )}
                      </div>
                    </section>
                  </div>

                  {/* Right Column: Drawdown & Pool Allocations */}
                  <div className="space-y-6">
                    <section className="bg-synod-card border border-synod-border rounded-md p-6">
                      <div className="flex justify-between items-end mb-4">
                        <h3 className="text-[11px] font-bold uppercase tracking-widest text-synod-muted">Drawdown Monitor</h3>
                        <span className="text-xs font-mono font-bold text-synod-warning">8.4%</span>
                      </div>
                      <div className="h-2 bg-zinc-900 rounded-full overflow-hidden relative">
                        <div className="h-full bg-white w-[42%] rounded-full transition-all duration-1000" />
                        <div className="absolute right-[20%] top-0 bottom-0 w-[1px] bg-red-400 opacity-50" />
                      </div>
                      <p className="text-[9px] text-synod-muted mt-4 leading-relaxed">
                        Limit: 20% · Auto-halt active. <br />
                        Peak AUM: ${state.peak_aum_usd.toLocaleString()}
                      </p>
                    </section>

                    <section className="bg-synod-card border border-synod-border rounded-md">
                      <div className="p-5 border-b border-synod-border flex justify-between items-center">
                        <h2 className="text-[11px] font-bold uppercase tracking-widest text-synod-muted">Pool Allocations</h2>
                        <button className="text-[9px] font-bold uppercase tracking-widest text-synod-muted hover:text-white transition-colors" onClick={() => setActiveTab('policy')}>Rules →</button>
                      </div>
                      <div className="p-6 space-y-6">
                        {state.pools.map(pool => (
                          <PoolItem key={pool.pool_key} pool={pool} />
                        ))}
                      </div>
                    </section>
                  </div>
                </div>
              </div>
            )}

            {activeTab === 'wallets' && (
              <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
                {state.wallets.map(wallet => (
                  <WalletCard
                    key={wallet.wallet_address}
                    treasuryId={state.treasury_id}
                    token={token}
                    wallet={wallet}
                    onBalanceUpdate={(addr, aum) => {
                      setWalletBalances(prev => ({ ...prev, [addr]: aum }))
                      setAumLoaded(true)
                    }}
                    onDisconnect={() => {
                      setWalletBalances(prev => {
                        const next = { ...prev };
                        delete next[wallet.wallet_address];
                        return next;
                      });
                      triggerResync();
                      fetchData();
                    }}
                  />
                ))}
                <WalletConnect
                  treasuryId={state.treasury_id}
                  token={token}
                  activeWallets={state.wallets || []}
                  onSuccess={() => {
                    triggerResync();
                    fetchData();
                  }}
                />
              </div>
            )}

            {activeTab === 'agents' && (
              <div className="space-y-8 h-full">
                <AgentManager
                  treasuryId={state.treasury_id}
                  token={token}
                  agents={agentsData}
                  onAgentsChange={() => fetchAgents(state.treasury_id)}
                  onManageRules={handleManageRules}
                  isDashboardWidget={false}
                />
              </div>
            )}

            {activeTab === 'policy' && (
              <PolicyManager
                treasuryId={state.treasury_id}
                token={token}
                wallets={state.wallets}
                agents={agentsData}
                treasuryHealth={state.health}
                focusAgentId={policyFocusAgentId}
                onTreasuryRefresh={fetchData}
                onOpenAgent={handleOpenAgentSlot}
              />
            )}

            {(activeTab === 'permits' || activeTab === 'activity' || activeTab === 'settings') && (
              <div className="py-20 text-center space-y-4 bg-synod-card border border-synod-border border-dashed rounded-md">
                <div className="text-sm font-bold text-white uppercase tracking-widest">Section Initialization Required</div>
                <p className="text-xs text-synod-muted">This module is currently being optimized for the new B&W architecture.</p>
              </div>
            )}
          </div>
        </main>
      </div>
    </div>
  )
}

function KPICard({ label, value, change, trend, isLoading, isCurrency }: KPICardProps) {
  const [displayVal, setDisplayVal] = useState(0)
  const targetVal = isCurrency && typeof value === "number" ? value : 0

  useEffect(() => {
    if (!isCurrency || isLoading) return
    // Animate count-up
    const duration = 1200
    const start = performance.now()
    const from = displayVal
    const to = targetVal
    function tick(now: number) {
      const elapsed = now - start
      const progress = Math.min(elapsed / duration, 1)
      // Ease out cubic
      const eased = 1 - Math.pow(1 - progress, 3)
      setDisplayVal(Math.round(from + (to - from) * eased))
      if (progress < 1) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }, [targetVal, isLoading])

  const formattedValue = isCurrency
    ? `$${displayVal.toLocaleString(undefined, { minimumFractionDigits: 0, maximumFractionDigits: 0 })}`
    : value

  return (
    <div className="bg-synod-card border border-synod-border p-5 rounded-md hover:border-synod-border-strong transition-colors cursor-default">
      <h4 className="text-[9px] font-mono text-synod-muted uppercase tracking-[0.2em] mb-4">{label}</h4>
      {isLoading ? (
        <div className="h-8 flex items-center">
          <div className="h-5 w-28 bg-gradient-to-r from-white/5 via-white/10 to-white/5 rounded animate-pulse" style={{ animationDuration: '1.5s' }} />
        </div>
      ) : (
        <div className="text-2xl font-bold text-white tracking-tighter">{formattedValue}</div>
      )}
      <div className={`text-[10px] font-mono flex items-center gap-1 mt-2 ${trend === 'up' ? 'text-white' : 'text-synod-muted'}`}>
        {trend === 'up' ? <ArrowUpRight size={12} /> : null}
        {change}
      </div>
    </div>
  )
}

function PoolItem({ pool }: { pool: Pool }) {
  const currentPct = 52.4; // Mock for now, would be computed
  return (
    <div className="group">
      <div className="flex justify-between items-end mb-2">
        <div>
          <span className="text-[13px] font-bold text-white tracking-tight">{pool.asset_code}</span>
          <span className="ml-2 text-[9px] font-mono text-synod-muted uppercase tracking-widest">{pool.pool_key}</span>
        </div>
        <div className="text-right">
          <span className="text-xs font-mono font-bold text-white">{currentPct}%</span>
          <span className="ml-2 text-[10px] font-mono text-synod-muted-dark tracking-tighter">/ target {pool.target_pct}%</span>
        </div>
      </div>
      <div className="h-1.5 bg-zinc-900 rounded-full overflow-visible relative">
        <div
          className="h-full bg-white rounded-full transition-all duration-700"
          style={{ width: `${currentPct}%` }}
        />
        <div
          className="absolute top-1/2 -translate-y-1/2 w-[2px] h-3 bg-zinc-700 rounded-full border border-black"
          style={{ left: `${pool.target_pct}%` }}
        />
      </div>
      <div className="flex gap-4 mt-3">
        <span className="text-[9px] font-mono text-synod-muted-dark uppercase tracking-widest">Drift <span className="text-white">+2.4%</span></span>
        <span className="text-[9px] font-mono text-synod-muted-dark uppercase tracking-widest">Value <span className="text-white">$130,244</span></span>
      </div>
    </div>
  )
}

function EventRow({ event }: { event: any }) {
  const typeStr = event?.event_type || event?.type || (typeof event === 'object' ? Object.keys(event)[0] : "EVENT");
  const payloadStr = event?.payload ? JSON.stringify(event.payload) : JSON.stringify(event);

  return (
    <div className="px-6 py-3.5 flex items-center justify-between hover:bg-white/[0.02] transition-colors group">
      <div className="flex items-center gap-6">
        <div className="w-1 h-1 bg-white rounded-full opacity-20 group-hover:opacity-100 transition-opacity" />
        <div>
          <div className="text-[11px] font-bold text-white uppercase tracking-wider">{typeStr}</div>
          <div className="text-[10px] font-mono text-synod-muted-dark mt-0.5 truncate max-w-md">
            {(payloadStr || "").substring(0, 80)}...
          </div>
        </div>
      </div>
      <div className="text-[9px] font-mono text-synod-muted-dark">
        {new Date().toLocaleTimeString()}
      </div>
    </div>
  )
}

function AgentRowMini({ name, status }: { name: string, status: string }) {
  return (
    <div className="flex items-center gap-4 group">
      <div className={`w-1.5 h-1.5 rounded-full ${status === 'active' ? 'bg-white shadow-[0_0_8px_rgba(255,255,255,0.4)]' : 'bg-red-500 opacity-50'}`} />
      <span className="text-xs font-medium text-white/80 group-hover:text-white transition-colors">{name}</span>
      <span className="ml-auto text-[9px] font-mono text-synod-muted-dark uppercase tracking-widest">{status}</span>
    </div>
  )
}
