"use client"

import { useEffect, useState } from "react"
import { AlertTriangle } from "lucide-react"
import { Button } from "@/components/ui/button"
import type { AgentSlot } from "@/components/dashboard/agent-manager"

interface WalletSummary {
  wallet_address: string
  label: string | null
  multisig_active: boolean
  status: string
}

interface TreasuryRules {
  max_drawdown_pct: number
  max_concurrent_permits: number
}

interface AgentWalletRule {
  agent_id: string
  wallet_address: string
  allocation_pct: number
  tier_limit_usd: number
  concurrent_permit_cap: number
}

interface ConstitutionContent {
  treasury_rules: TreasuryRules
  agent_wallet_rules: AgentWalletRule[]
  memo: string | null
}

interface ConstitutionResponse {
  version: number
  treasury_id: string
  state_hash: string
  content: ConstitutionContent
  executed_at: string
}

interface PolicyManagerProps {
  treasuryId: string
  token: string | null
  wallets: WalletSummary[]
  agents: AgentSlot[]
  treasuryHealth: "HEALTHY" | "HALTED" | "DEGRADED" | "PENDING_WALLET"
  focusAgentId?: string | null
  onTreasuryRefresh?: () => void | Promise<void>
  onOpenAgent?: (agentId: string) => void
}

interface RuleDraft {
  allocation_pct: string
  tier_limit_usd: string
  concurrent_permit_cap: string
}

interface RevokeTarget {
  agentId: string
  walletAddress: string
  agentName: string
  walletLabel: string
}

function truncateMiddle(value: string, left = 6, right = 4) {
  if (value.length <= left + right + 3) return value
  return `${value.slice(0, left)}...${value.slice(-right)}`
}

function statusClasses(status: string) {
  if (status === "ACTIVE") return "border-emerald-500/25 bg-emerald-500/10 text-emerald-300"
  if (status.startsWith("PENDING")) return "border-synod-warning/30 bg-synod-warning/10 text-synod-warning"
  if (status === "INACTIVE") return "border-zinc-700 bg-zinc-900 text-zinc-300"
  if (status === "REVOKED") return "border-red-500/25 bg-red-500/10 text-red-300"
  if (status === "SUSPENDED") return "border-amber-500/25 bg-amber-500/10 text-amber-200"
  return "border-sky-500/25 bg-sky-500/10 text-sky-200"
}

function buildCellKey(agentId: string, walletAddress: string) {
  return `${agentId}:${walletAddress}`
}

function formatDate(value: string | null) {
  if (!value) return "Never"
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return "Never"
  return date.toLocaleString()
}

export function PolicyManager({
  treasuryId,
  token,
  wallets,
  agents,
  treasuryHealth,
  focusAgentId,
  onTreasuryRefresh,
  onOpenAgent,
}: PolicyManagerProps) {
  const [constitution, setConstitution] = useState<ConstitutionResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState("")
  const [notice, setNotice] = useState("")
  const [maxDrawdown, setMaxDrawdown] = useState("")
  const [maxConcurrentPermits, setMaxConcurrentPermits] = useState("")
  const [editingCell, setEditingCell] = useState<string | null>(null)
  const [draft, setDraft] = useState<RuleDraft>({ allocation_pct: "", tier_limit_usd: "", concurrent_permit_cap: "" })
  const [revokeTarget, setRevokeTarget] = useState<RevokeTarget | null>(null)
  const [showResumeConfirm, setShowResumeConfirm] = useState(false)

  useEffect(() => {
    const loadConstitution = async () => {
      if (!token) return

      setLoading(true)
      setError("")

      try {
        const res = await fetch(`/v1/treasuries/${treasuryId}/constitution`, {
          headers: { Authorization: `Bearer ${token}` },
        })

        if (!res.ok) {
          const data = await res.json().catch(() => null)
          throw new Error(data?.message || "Failed to load constitution")
        }

        const data: ConstitutionResponse = await res.json()
        setConstitution(data)
        setMaxDrawdown(String(data.content.treasury_rules.max_drawdown_pct))
        setMaxConcurrentPermits(String(data.content.treasury_rules.max_concurrent_permits))
      } catch (err) {
        console.error(err)
        setError(err instanceof Error ? err.message : "Failed to load constitution")
      } finally {
        setLoading(false)
      }
    }

    loadConstitution()
  }, [token, treasuryId])

  useEffect(() => {
    if (!focusAgentId) return

    const row = document.querySelector(`[data-agent-row="${focusAgentId}"]`)
    if (row instanceof HTMLElement) {
      row.scrollIntoView({ block: "center", behavior: "smooth" })
    }
  }, [focusAgentId, constitution])

  const saveConstitution = async (content: ConstitutionContent, successMessage: string) => {
    if (!token) return

    setSaving(true)
    setError("")
    setNotice("")

    try {
      const res = await fetch(`/v1/treasuries/${treasuryId}/constitution`, {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ content }),
      })

      if (!res.ok) {
        const data = await res.json().catch(() => null)
        throw new Error(data?.message || "Failed to save constitution")
      }

      const data: ConstitutionResponse = await res.json()
      setConstitution(data)
      setMaxDrawdown(String(data.content.treasury_rules.max_drawdown_pct))
      setMaxConcurrentPermits(String(data.content.treasury_rules.max_concurrent_permits))
      setNotice(`${successMessage} Constitution v${data.version} saved.`)
      setEditingCell(null)
      setRevokeTarget(null)
    } catch (err) {
      console.error(err)
      setError(err instanceof Error ? err.message : "Failed to save constitution")
    } finally {
      setSaving(false)
    }
  }

  const getRule = (agentId: string, walletAddress: string) => {
    return constitution?.content.agent_wallet_rules.find(
      (rule) => rule.agent_id === agentId && rule.wallet_address === walletAddress,
    )
  }

  const startEditing = (agentId: string, walletAddress: string) => {
    const existing = getRule(agentId, walletAddress)
    setEditingCell(buildCellKey(agentId, walletAddress))
    setDraft({
      allocation_pct: existing ? String(existing.allocation_pct) : "",
      tier_limit_usd: existing ? String(existing.tier_limit_usd) : "",
      concurrent_permit_cap: existing ? String(existing.concurrent_permit_cap) : "1",
    })
    setError("")
    setNotice("")
  }

  const walletAllocationSnapshot = (walletAddress: string, editingAgentId?: string) => {
    const otherRules = (constitution?.content.agent_wallet_rules ?? []).filter(
      (rule) => rule.wallet_address === walletAddress && rule.agent_id !== editingAgentId,
    )
    const allocated = otherRules.reduce((sum, rule) => sum + rule.allocation_pct, 0)
    const draftAllocation = Number.parseFloat(draft.allocation_pct || "0")
    const total = allocated + (Number.isFinite(draftAllocation) ? draftAllocation : 0)
    return {
      allocated,
      total,
      remaining: Math.max(0, 100 - total),
      warning: total >= 90,
      exceeds: total > 100,
    }
  }

  const saveTreasuryRules = async () => {
    if (!constitution) return

    const nextDrawdown = Number.parseFloat(maxDrawdown)
    const nextConcurrent = Number.parseInt(maxConcurrentPermits, 10)

    if (!Number.isFinite(nextDrawdown) || nextDrawdown <= 0) {
      setError("Maximum Drawdown % must be greater than 0.")
      return
    }

    if (!Number.isInteger(nextConcurrent) || nextConcurrent < 1) {
      setError("Maximum Concurrent Permits must be at least 1.")
      return
    }

    await saveConstitution(
      {
        ...constitution.content,
        treasury_rules: {
          max_drawdown_pct: nextDrawdown,
          max_concurrent_permits: nextConcurrent,
        },
      },
      "Treasury rules updated.",
    )
  }

  const saveAccessRule = async (agentId: string, walletAddress: string) => {
    if (!constitution) return

    const allocation = Number.parseFloat(draft.allocation_pct)
    const tierLimit = Number.parseFloat(draft.tier_limit_usd)
    const concurrentCap = Number.parseInt(draft.concurrent_permit_cap, 10)

    if (!Number.isFinite(allocation) || allocation < 1 || allocation > 100) {
      setError("Allocation % must be between 1 and 100.")
      return
    }

    if (!Number.isFinite(tierLimit) || tierLimit <= 0) {
      setError("Tier Limit USD must be greater than 0.")
      return
    }

    if (!Number.isInteger(concurrentCap) || concurrentCap < 1) {
      setError("Concurrent Permit Cap must be an integer greater than or equal to 1.")
      return
    }

    const totals = walletAllocationSnapshot(walletAddress, agentId)
    if (totals.exceeds) {
      setError(`Wallet allocation exceeds 100%. ${totals.remaining.toFixed(0)}% remains available.`)
      return
    }

    const nextRules = (constitution.content.agent_wallet_rules ?? []).filter(
      (rule) => !(rule.agent_id === agentId && rule.wallet_address === walletAddress),
    )

    nextRules.push({
      agent_id: agentId,
      wallet_address: walletAddress,
      allocation_pct: allocation,
      tier_limit_usd: tierLimit,
      concurrent_permit_cap: concurrentCap,
    })

    await saveConstitution(
      {
        ...constitution.content,
        agent_wallet_rules: nextRules,
      },
      "Agent access updated.",
    )
  }

  const revokeAccessRule = async () => {
    if (!constitution || !revokeTarget) return

    const nextRules = constitution.content.agent_wallet_rules.filter(
      (rule) => !(rule.agent_id === revokeTarget.agentId && rule.wallet_address === revokeTarget.walletAddress),
    )

    await saveConstitution(
      {
        ...constitution.content,
        agent_wallet_rules: nextRules,
      },
      `Removed ${revokeTarget.agentName} access from ${revokeTarget.walletLabel}.`,
    )
  }

  const resumeTreasury = async () => {
    if (!token) return

    setSaving(true)
    setError("")

    try {
      const res = await fetch(`/v1/treasuries/${treasuryId}/resume`, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
      })

      if (!res.ok) {
        const data = await res.json().catch(() => null)
        throw new Error(data?.message || "Failed to resume treasury")
      }

      setShowResumeConfirm(false)
      setNotice("Treasury resumed successfully.")
      await Promise.resolve(onTreasuryRefresh?.())
    } catch (err) {
      console.error(err)
      setError(err instanceof Error ? err.message : "Failed to resume treasury")
    } finally {
      setSaving(false)
    }
  }

  if (loading) {
    return (
      <div className="py-20 text-center space-y-4 bg-synod-card border border-synod-border rounded-md">
        <div className="text-sm font-bold text-white uppercase tracking-widest">Loading Policy</div>
        <p className="text-xs text-synod-muted">Fetching the current constitution and access matrix.</p>
      </div>
    )
  }

  if (error && !constitution) {
    return (
      <div className="py-20 text-center space-y-4 bg-synod-card border border-red-500/20 rounded-md">
        <div className="text-sm font-bold text-white uppercase tracking-widest">Policy Load Failed</div>
        <p className="text-xs text-red-200">{error}</p>
      </div>
    )
  }

  if (!constitution) {
    return null
  }

  const noWallets = wallets.length === 0
  const noAgents = agents.length === 0
  const noRulesConfigured = constitution.content.agent_wallet_rules.length === 0

  return (
    <div className="space-y-6">
      <section className="rounded-md border border-synod-border bg-synod-card">
        <div className="flex flex-col gap-4 border-b border-synod-border px-5 py-4 lg:flex-row lg:items-end lg:justify-between">
          <div>
            <div className="text-sm font-bold text-white">Treasury Rules</div>
            <div className="mt-1 text-[10px] uppercase tracking-[0.18em] text-synod-muted">
              Constitution v{constitution.version} - last updated {formatDate(constitution.executed_at)}
            </div>
          </div>
          <div className="text-[10px] font-mono uppercase tracking-[0.16em] text-synod-muted-dark break-all">
            {constitution.state_hash}
          </div>
        </div>

        <div className="space-y-5 p-5">
          {treasuryHealth === "HALTED" && (
            <div className="flex flex-col gap-4 rounded-xl border border-red-500/20 bg-red-500/10 px-4 py-4 lg:flex-row lg:items-center lg:justify-between">
              <div>
                <div className="text-sm font-bold text-white">Treasury is halted. Capital is frozen.</div>
                <p className="mt-2 text-sm text-synod-muted">Resume operations below when you are ready to restore capital movement.</p>
              </div>
              <Button type="button" variant="error" size="sm" onClick={() => setShowResumeConfirm(true)} className="h-10 px-4 text-[10px] font-bold uppercase tracking-[0.16em]">
                Resume
              </Button>
            </div>
          )}

          {(error || notice) && (
            <div className={`rounded-xl px-4 py-3 text-xs ${error ? "border border-red-500/25 bg-red-500/10 text-red-200" : "border border-emerald-500/25 bg-emerald-500/10 text-emerald-200"}`}>
              {error || notice}
            </div>
          )}

          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted-dark">Maximum Drawdown %</label>
              <input
                type="number"
                min="0"
                step="0.1"
                value={maxDrawdown}
                onChange={(event) => setMaxDrawdown(event.target.value)}
                className="h-12 w-full rounded-xl border border-synod-border bg-black px-4 text-sm text-white outline-none transition-colors focus:border-white"
              />
            </div>

            <div className="space-y-2">
              <label className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted-dark">Maximum Concurrent Permits</label>
              <input
                type="number"
                min="1"
                step="1"
                value={maxConcurrentPermits}
                onChange={(event) => setMaxConcurrentPermits(event.target.value)}
                className="h-12 w-full rounded-xl border border-synod-border bg-black px-4 text-sm text-white outline-none transition-colors focus:border-white"
              />
            </div>
          </div>

          <div className="flex justify-end">
            <Button type="button" variant="secondary" size="sm" disabled={saving} onClick={saveTreasuryRules} className="h-10 px-4 text-[10px] font-bold uppercase tracking-[0.16em]">
              {saving ? "Saving..." : "Save Treasury Rules"}
            </Button>
          </div>
        </div>
      </section>

      <section className="rounded-md border border-synod-border bg-synod-card">
        <div className="border-b border-synod-border px-5 py-4">
          <div className="text-sm font-bold text-white">Agent Rules</div>
          <div className="mt-1 text-[10px] uppercase tracking-[0.18em] text-synod-muted">Policy is the only source of truth for agent wallet access.</div>
        </div>

        <div className="space-y-4 p-5">
          {noAgents ? (
            <div className="rounded-md border border-synod-border bg-black px-4 py-8 text-center text-sm text-synod-muted">
              No agents yet. Create an agent slot first, then configure its access to wallets here.
            </div>
          ) : noWallets ? (
            <div className="rounded-md border border-synod-border bg-black px-4 py-8 text-center text-sm text-synod-muted">
              No wallets connected. Connect a wallet from the Wallets page to start configuring agent access.
            </div>
          ) : (
            <>
              {noRulesConfigured && (
                <div className="rounded-md border border-synod-border bg-black px-4 py-3 text-sm text-synod-muted">
                  No access rules configured. Agents cannot move capital until you grant wallet access here.
                </div>
              )}

              <div className="custom-scrollbar overflow-x-auto">
                <table className="min-w-full border-collapse">
                  <thead>
                    <tr className="border-b border-synod-border text-left">
                      <th className="sticky left-0 z-10 min-w-[220px] bg-synod-card px-4 py-3 text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">Agent</th>
                      {wallets.map((wallet) => (
                        <th key={wallet.wallet_address} className="min-w-[220px] px-4 py-3 text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">
                          <div>{wallet.label || "Wallet"}</div>
                          <div className="mt-1 font-mono text-[10px] normal-case tracking-normal text-synod-muted-dark">{truncateMiddle(wallet.wallet_address, 8, 4)}</div>
                        </th>
                      ))}
                      <th className="min-w-[140px] px-4 py-3 text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">Actions</th>
                    </tr>
                  </thead>

                  <tbody>
                    {agents.map((agent) => (
                      <tr
                        key={agent.agent_id}
                        data-agent-row={agent.agent_id}
                        className={`border-b border-synod-border/70 align-top ${focusAgentId === agent.agent_id ? "bg-white/[0.04]" : ""}`}
                      >
                        <td className="sticky left-0 z-10 bg-synod-card px-4 py-4">
                          <div className="text-sm font-bold text-white">{agent.name}</div>
                          <div className="mt-1 font-mono text-[10px] text-synod-muted-dark">{truncateMiddle(agent.agent_id, 8, 4)}</div>
                          <span className={`mt-3 inline-flex items-center rounded-full border px-2 py-1 text-[8px] font-bold uppercase tracking-[0.16em] ${statusClasses(agent.status)}`}>
                            {agent.status.startsWith("PENDING") ? "PENDING" : agent.status}
                          </span>
                        </td>

                        {wallets.map((wallet) => {
                          const cellKey = buildCellKey(agent.agent_id, wallet.wallet_address)
                          const existingRule = getRule(agent.agent_id, wallet.wallet_address)
                          const isEditing = editingCell === cellKey
                          const totals = walletAllocationSnapshot(wallet.wallet_address, agent.agent_id)

                          return (
                            <td key={wallet.wallet_address} className="px-4 py-4">
                              {isEditing ? (
                                <div className="space-y-3 rounded-md border border-synod-border bg-black p-4">
                                  <div className="space-y-2">
                                    <label className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Allocation %</label>
                                    <input
                                      type="number"
                                      min="1"
                                      max="100"
                                      step="0.1"
                                      value={draft.allocation_pct}
                                      onChange={(event) => setDraft((current) => ({ ...current, allocation_pct: event.target.value }))}
                                      className="h-10 w-full rounded-md border border-synod-border bg-zinc-950 px-3 text-sm text-white outline-none focus:border-white"
                                    />
                                  </div>
                                  <div className="space-y-2">
                                    <label className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Tier Limit USD</label>
                                    <input
                                      type="number"
                                      min="0"
                                      step="0.01"
                                      value={draft.tier_limit_usd}
                                      onChange={(event) => setDraft((current) => ({ ...current, tier_limit_usd: event.target.value }))}
                                      className="h-10 w-full rounded-md border border-synod-border bg-zinc-950 px-3 text-sm text-white outline-none focus:border-white"
                                    />
                                  </div>
                                  <div className="space-y-2">
                                    <label className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Concurrent Permit Cap</label>
                                    <input
                                      type="number"
                                      min="1"
                                      step="1"
                                      value={draft.concurrent_permit_cap}
                                      onChange={(event) => setDraft((current) => ({ ...current, concurrent_permit_cap: event.target.value }))}
                                      className="h-10 w-full rounded-md border border-synod-border bg-zinc-950 px-3 text-sm text-white outline-none focus:border-white"
                                    />
                                  </div>

                                  <div className={`rounded-md px-3 py-2 text-[10px] ${totals.exceeds ? "border border-red-500/25 bg-red-500/10 text-red-200" : totals.warning ? "border border-amber-500/20 bg-amber-500/10 text-amber-100" : "border border-synod-border bg-white/[0.02] text-synod-muted"}`}>
                                    {totals.allocated.toFixed(0)}% allocated across other agents. {Math.max(0, 100 - totals.total).toFixed(0)}% available.
                                  </div>

                                  <div className="flex gap-2">
                                    <Button type="button" variant="secondary" size="sm" disabled={saving} onClick={() => saveAccessRule(agent.agent_id, wallet.wallet_address)} className="h-9 px-3 text-[10px] font-bold uppercase tracking-[0.16em]">
                                      Save
                                    </Button>
                                    <Button type="button" variant="ghost" size="sm" onClick={() => setEditingCell(null)} className="h-9 px-3 text-[10px] font-bold uppercase tracking-[0.16em]">
                                      Cancel
                                    </Button>
                                  </div>
                                </div>
                              ) : existingRule ? (
                                <div className="space-y-3 rounded-md border border-synod-border bg-black p-4">
                                  <div className="space-y-1 text-sm text-white">
                                    <div>{existingRule.allocation_pct}% allocation</div>
                                    <div>${existingRule.tier_limit_usd.toLocaleString()} per permit</div>
                                    <div>{existingRule.concurrent_permit_cap} concurrent</div>
                                  </div>
                                  <div className="flex gap-2">
                                    <Button type="button" variant="ghost" size="sm" onClick={() => startEditing(agent.agent_id, wallet.wallet_address)} className="h-8 border border-synod-border px-3 text-[10px] font-bold uppercase tracking-[0.16em] text-white">
                                      Edit
                                    </Button>
                                    <button
                                      type="button"
                                      onClick={() => setRevokeTarget({
                                        agentId: agent.agent_id,
                                        walletAddress: wallet.wallet_address,
                                        agentName: agent.name,
                                        walletLabel: wallet.label || truncateMiddle(wallet.wallet_address, 8, 4),
                                      })}
                                      className="inline-flex h-8 items-center justify-center rounded-md bg-red-600 px-3 text-[10px] font-bold uppercase tracking-[0.16em] text-white transition-colors hover:bg-red-500"
                                    >
                                      Revoke
                                    </button>
                                  </div>
                                </div>
                              ) : (
                                <button
                                  type="button"
                                  onClick={() => startEditing(agent.agent_id, wallet.wallet_address)}
                                  className="inline-flex h-11 items-center justify-center rounded-md border border-dashed border-synod-border bg-black px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-synod-muted transition-colors hover:border-white hover:text-white"
                                >
                                  + Grant Access
                                </button>
                              )}
                            </td>
                          )
                        })}

                        <td className="px-4 py-4">
                          {onOpenAgent ? (
                            <Button type="button" variant="ghost" size="sm" onClick={() => onOpenAgent(agent.agent_id)} className="h-9 border border-synod-border px-3 text-[10px] font-bold uppercase tracking-[0.16em] text-white">
                              View Slot
                            </Button>
                          ) : null}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}
        </div>
      </section>

      {showResumeConfirm && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-2xl border border-synod-border bg-[#07070b] shadow-2xl">
            <div className="border-b border-synod-border px-6 py-5 text-xl font-bold text-white">Resume Treasury</div>
            <div className="space-y-5 px-6 py-6">
              <div className="flex items-start gap-3 rounded-xl border border-amber-500/20 bg-amber-500/10 px-4 py-4">
                <AlertTriangle size={18} className="mt-0.5 shrink-0 text-amber-200" />
                <p className="text-sm leading-6 text-synod-muted">
                  Resume operations and unfreeze capital? This will allow permit issuance to continue under the current constitution.
                </p>
              </div>
            </div>
            <div className="flex justify-end gap-3 px-6 pb-6">
              <Button type="button" variant="ghost" size="sm" onClick={() => setShowResumeConfirm(false)} className="h-10 border border-synod-border px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-white">
                Cancel
              </Button>
              <button
                type="button"
                onClick={resumeTreasury}
                disabled={saving}
                className="inline-flex h-10 items-center justify-center rounded-lg bg-white px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-black transition-colors hover:bg-zinc-200 disabled:opacity-60"
              >
                {saving ? "Resuming..." : "Confirm Resume"}
              </button>
            </div>
          </div>
        </div>
      )}

      {revokeTarget && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-2xl border border-synod-border bg-[#07070b] shadow-2xl">
            <div className="border-b border-synod-border px-6 py-5 text-xl font-bold text-white">Confirm Access Removal</div>
            <div className="space-y-5 px-6 py-6">
              <div className="flex items-start gap-3 rounded-xl border border-red-500/20 bg-red-500/10 px-4 py-4">
                <AlertTriangle size={18} className="mt-0.5 shrink-0 text-red-300" />
                <p className="text-sm leading-6 text-synod-muted">
                  Remove {revokeTarget.agentName}&apos;s access to {revokeTarget.walletLabel}? This will prevent the agent from requesting any permits against this wallet immediately.
                </p>
              </div>
            </div>
            <div className="flex justify-end gap-3 px-6 pb-6">
              <Button type="button" variant="ghost" size="sm" onClick={() => setRevokeTarget(null)} className="h-10 border border-synod-border px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-white">
                Cancel
              </Button>
              <button
                type="button"
                onClick={revokeAccessRule}
                disabled={saving}
                className="inline-flex h-10 items-center justify-center rounded-lg bg-red-600 px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-white transition-colors hover:bg-red-500 disabled:opacity-60"
              >
                {saving ? "Removing..." : "Confirm Revoke"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
