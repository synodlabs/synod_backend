"use client"

import { useEffect, useMemo, useState } from "react"
import { AlertTriangle, CheckCircle2, Copy, Plus, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useStellarWallet } from "@/hooks/use-stellar-wallet"

export interface AgentSlot {
  agent_id: string
  treasury_id: string
  name: string
  description: string | null
  agent_pubkey: string | null
  status: string
  created_at: string
  last_connected: string | null
}

interface AgentManagerProps {
  treasuryId: string
  token: string | null
  agents: AgentSlot[]
  onAgentsChange: () => void | Promise<void>
  onManageRules?: (agentId: string) => void
  isDashboardWidget?: boolean
}

type ProvisionResult = { agent: AgentSlot }
type ProvisionStep = "form" | "success"
type SdkTab = "python" | "nodejs" | "rust"
type CopyTarget = "agent_id" | "agent_pubkey" | null
type SnippetTone = "comment" | "command" | "env" | "keyword" | "call" | "string" | "number" | "plain"

type SnippetLine = {
  text: string
  tone: SnippetTone
}

const SDK_SNIPPETS: Record<SdkTab, { language: string; lines: SnippetLine[] }> = {
  python: {
    language: "Python",
    lines: [
      { text: "# Install", tone: "comment" },
      { text: "pip install synod-sdk", tone: "command" },
      { text: "", tone: "plain" },
      { text: "# Generate local identity", tone: "comment" },
      { text: "from synod import SynodAgent", tone: "keyword" },
      { text: "", tone: "plain" },
      { text: "agent = SynodAgent(", tone: "plain" },
      { text: '    key_storage_path="./synod_keys",', tone: "string" },
      { text: ")", tone: "plain" },
      { text: "", tone: "plain" },
      { text: 'print("Enroll this key in Synod:", agent.public_key)', tone: "call" },
      { text: "", tone: "plain" },
      { text: "# Connect after the dashboard binds the key", tone: "comment" },
      { text: "await agent.connect()", tone: "call" },
      { text: "await agent.execute(", tone: "call" },
      { text: '    wallet="G...",', tone: "string" },
      { text: '    destination="G...",', tone: "string" },
      { text: "    amount=250.0,", tone: "number" },
      { text: '    asset="XLM",', tone: "string" },
      { text: ")", tone: "plain" },
    ],
  },
  nodejs: {
    language: "Node.js",
    lines: [
      { text: "// Node.js SDK", tone: "comment" },
      { text: "// Python is the primary implementation today.", tone: "comment" },
      { text: "", tone: "plain" },
      { text: "const agent = new SynodAgent({ keyStoragePath: './synod_keys' })", tone: "keyword" },
      { text: "console.log(agent.publicKey)", tone: "call" },
      { text: "await agent.connect()", tone: "call" },
      { text: "await agent.execute({ wallet, destination, amount, asset })", tone: "call" },
    ],
  },
  rust: {
    language: "Rust",
    lines: [
      { text: "// Rust SDK", tone: "comment" },
      { text: "// Python is the primary implementation today.", tone: "comment" },
      { text: "", tone: "plain" },
      { text: 'let agent = SynodAgent::new("./synod_keys")?;', tone: "keyword" },
      { text: 'println!("{}", agent.public_key());', tone: "call" },
      { text: "agent.connect().await?;", tone: "call" },
      { text: "agent.execute(wallet, destination, amount, asset).await?;", tone: "call" },
    ],
  },
}

function formatRelativeTime(value: string | null) {
  if (!value) return "Never connected"

  const timestamp = new Date(value).getTime()
  if (Number.isNaN(timestamp)) return "Never connected"

  const diffMs = Date.now() - timestamp
  const diffMinutes = Math.floor(diffMs / 60000)

  if (diffMinutes < 1) return "Just now"
  if (diffMinutes < 60) return `${diffMinutes}m ago`

  const diffHours = Math.floor(diffMinutes / 60)
  if (diffHours < 24) return `${diffHours}h ago`

  const diffDays = Math.floor(diffHours / 24)
  return `${diffDays}d ago`
}

function formatDate(value: string | null) {
  if (!value) return "Never connected"

  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return "Never connected"

  return date.toLocaleString()
}

function displayStatus(status: string) {
  if (status.startsWith("PENDING")) return "PENDING"
  return status
}

function statusClasses(status: string) {
  if (status === "ACTIVE") return "border-emerald-500/25 bg-emerald-500/10 text-emerald-300"
  if (status.startsWith("PENDING")) return "border-synod-warning/30 bg-synod-warning/10 text-synod-warning"
  if (status === "INACTIVE") return "border-zinc-700 bg-zinc-900 text-zinc-300"
  if (status === "REVOKED") return "border-red-500/25 bg-red-500/10 text-red-300"
  if (status === "SUSPENDED") return "border-amber-500/25 bg-amber-500/10 text-amber-200"
  return "border-sky-500/25 bg-sky-500/10 text-sky-200"
}

function buildAgentToken(agentId: string) {
  const compact = agentId.replace(/-/g, "")
  return `${compact.slice(0, 8)}...${compact.slice(-4)}`
}

function snippetToneClass(tone: SnippetTone) {
  switch (tone) {
    case "comment":
      return "text-synod-muted-dark"
    case "command":
      return "text-white"
    case "env":
      return "text-synod-warning"
    case "keyword":
      return "text-emerald-300"
    case "call":
      return "text-sky-300"
    case "string":
      return "text-amber-200"
    case "number":
      return "text-fuchsia-300"
    default:
      return "text-synod-muted"
  }
}

function MiniAgentList({ agents }: { agents: AgentSlot[] }) {
  return (
    <section className="bg-synod-card border border-synod-border rounded-md overflow-hidden flex flex-col">
      <div className="p-5 border-b border-synod-border flex justify-between items-center bg-white/[0.01]">
        <div>
          <h2 className="text-sm font-bold text-white tracking-tight">Connected Agents</h2>
          <p className="text-[10px] text-synod-muted uppercase tracking-widest mt-1">
            {agents.length} configured slots
          </p>
        </div>
      </div>

      <div className="custom-scrollbar overflow-y-auto max-h-[258px]" style={{ scrollbarGutter: "stable" }}>
        <div className="divide-y divide-synod-border">
          {agents.length === 0 ? (
            <div className="py-20 text-center bg-synod-card/50">
              <p className="text-[10px] text-synod-muted font-mono uppercase tracking-[0.2em]">
                No Agent Slots
              </p>
            </div>
          ) : (
            agents.map((agent) => (
              <div key={agent.agent_id} className="p-4 flex items-center gap-4 hover:bg-white/[0.015] transition-all">
                <div
                  className={`relative w-2 h-2 rounded-full flex-shrink-0 ${
                    agent.status === "ACTIVE" ? "bg-white shadow-[0_0_8px_rgba(255,255,255,0.4)]" : "bg-synod-warning opacity-60"
                  }`}
                >
                  {agent.status === "ACTIVE" && <div className="absolute inset-0 rounded-full animate-ping bg-white opacity-20" />}
                </div>

                <div className="flex-1 min-w-0">
                  <div className="text-xs font-bold text-white truncate">{agent.name}</div>
                  <div className="text-[10px] font-mono text-synod-muted-dark truncate mt-0.5">
                    {buildAgentToken(agent.agent_id)} - {displayStatus(agent.status).toLowerCase()}
                  </div>
                </div>

                <div className="text-[10px] font-mono text-synod-muted uppercase tracking-tighter">
                  {formatRelativeTime(agent.last_connected)}
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </section>
  )
}

export function AgentManager({
  treasuryId,
  token,
  agents,
  onAgentsChange,
  onManageRules,
  isDashboardWidget = true,
}: AgentManagerProps) {
  const { connect: connectWallet, sign: signMessage } = useStellarWallet()
  const [showProvisionModal, setShowProvisionModal] = useState(false)
  const [showRevokeModal, setShowRevokeModal] = useState(false)
  const [isProvisioning, setIsProvisioning] = useState(false)
  const [newAgentName, setNewAgentName] = useState("")
  const [newAgentDescription, setNewAgentDescription] = useState("")
  const [provisionResult, setProvisionResult] = useState<ProvisionResult | null>(null)
  const [provisionStep, setProvisionStep] = useState<ProvisionStep>("form")
  const [copiedTarget, setCopiedTarget] = useState<CopyTarget>(null)
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null)
  const [actionLoading, setActionLoading] = useState<string | null>(null)
  const [sdkTab, setSdkTab] = useState<SdkTab>("python")
  const [modalError, setModalError] = useState("")
  const [revokeTarget, setRevokeTarget] = useState<AgentSlot | null>(null)
  const [enrollmentPubkey, setEnrollmentPubkey] = useState("")
  const [bindingAgentId, setBindingAgentId] = useState<string | null>(null)
  const [bindingStatus, setBindingStatus] = useState("")
  const [bindingError, setBindingError] = useState("")

  useEffect(() => {
    if (agents.length === 0) {
      setSelectedAgentId(null)
      return
    }

    const stillExists = selectedAgentId && agents.some((agent) => agent.agent_id === selectedAgentId)
    if (!stillExists) {
      setSelectedAgentId(agents[0].agent_id)
    }
  }, [agents, selectedAgentId])

  const selectedAgent = useMemo(() => {
    const found = agents.find((agent) => agent.agent_id === selectedAgentId)
    if (found) return found

    if (provisionResult && provisionResult.agent.agent_id === selectedAgentId) {
      return provisionResult.agent
    }

    return agents[0] ?? null
  }, [agents, provisionResult, selectedAgentId])

  useEffect(() => {
    if (!selectedAgent) return
    setEnrollmentPubkey(selectedAgent.agent_pubkey ?? "")
    setBindingStatus("")
    setBindingError("")
  }, [selectedAgent])

  const resetProvisionModal = () => {
    setShowProvisionModal(false)
    setProvisionResult(null)
    setProvisionStep("form")
    setNewAgentName("")
    setNewAgentDescription("")
    setModalError("")
    setCopiedTarget(null)
  }

  const handleProvision = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!token || !newAgentName.trim()) return

    setIsProvisioning(true)
    setModalError("")

    try {
      const res = await fetch(`/v1/agents/${treasuryId}`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          name: newAgentName.trim(),
          description: newAgentDescription.trim() || null,
        }),
      })

      if (!res.ok) {
        const data = await res.json().catch(() => null)
        throw new Error(data?.message || "Failed to provision agent slot")
      }

      const result: ProvisionResult = await res.json()
      setProvisionResult(result)
      setProvisionStep("success")
      setSelectedAgentId(result.agent.agent_id)
      await Promise.resolve(onAgentsChange())
    } catch (err) {
      console.error(err)
      setModalError(err instanceof Error ? err.message : "Provisioning failed. Please try again.")
    } finally {
      setIsProvisioning(false)
    }
  }

  const handleRevoke = async () => {
    if (!token || !revokeTarget) return

    setActionLoading(revokeTarget.agent_id)
    setModalError("")

    try {
      const res = await fetch(`/v1/agents/${treasuryId}/${revokeTarget.agent_id}/revoke`, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
      })

      if (!res.ok) {
        const data = await res.json().catch(() => null)
        throw new Error(data?.message || "Failed to revoke agent")
      }

      setShowRevokeModal(false)
      setRevokeTarget(null)
      await Promise.resolve(onAgentsChange())
    } catch (err) {
      console.error(err)
      setModalError(err instanceof Error ? err.message : "Revocation failed. Please try again.")
    } finally {
      setActionLoading(null)
    }
  }

  const handleBindPublicKey = async () => {
    if (!token || !selectedAgent || !enrollmentPubkey.trim()) return

    setBindingAgentId(selectedAgent.agent_id)
    setBindingStatus("Connecting wallet...")
    setBindingError("")

    try {
      const walletAddress = await connectWallet()
      if (!walletAddress) {
        setBindingStatus("")
        return
      }

      setBindingStatus("Requesting enrollment challenge...")
      const challengeRes = await fetch(`/v1/agents/${selectedAgent.agent_id}/enroll/challenge`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          wallet_address: walletAddress,
          agent_pubkey: enrollmentPubkey.trim(),
        }),
      })

      if (!challengeRes.ok) {
        const data = await challengeRes.json().catch(() => null)
        throw new Error(data?.message || "Failed to create enrollment challenge")
      }

      const challengeData = await challengeRes.json()
      const challengeMessage = `synod-enroll:${challengeData.agent_id}:${walletAddress}:${enrollmentPubkey.trim()}:${challengeData.challenge}`

      setBindingStatus("Sign the approval message in your wallet...")
      const signature = await signMessage(challengeMessage, walletAddress)
      if (!signature) {
        throw new Error("Wallet signing rejected")
      }

      setBindingStatus("Binding public key to this slot...")
      const enrollRes = await fetch(`/v1/agents/${selectedAgent.agent_id}/enroll-pubkey`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          wallet_address: walletAddress,
          agent_pubkey: enrollmentPubkey.trim(),
          challenge: challengeData.challenge,
          signature,
        }),
      })

      if (!enrollRes.ok) {
        const data = await enrollRes.json().catch(() => null)
        throw new Error(data?.message || "Failed to bind agent public key")
      }

      setBindingStatus("Agent key enrolled successfully.")
      await Promise.resolve(onAgentsChange())
    } catch (err) {
      console.error(err)
      setBindingError(err instanceof Error ? err.message : "Failed to bind public key")
      setBindingStatus("")
    } finally {
      setBindingAgentId(null)
    }
  }

  const copyToClipboard = async (text: string, target: CopyTarget) => {
    await navigator.clipboard.writeText(text)
    setCopiedTarget(target)
    setTimeout(() => setCopiedTarget(null), 2000)
  }

  if (isDashboardWidget) {
    return <MiniAgentList agents={agents} />
  }

  return (
    <>
      <div className="space-y-6">
        <div className="flex justify-end">
          <div className="flex items-center gap-3">
            <Button
              variant="ghost"
              size="sm"
              className="h-9 border border-synod-border bg-white/[0.02] px-4 text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted hover:border-synod-border-strong hover:text-white"
            >
              Docs
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={() => {
                setShowProvisionModal(true)
                setProvisionResult(null)
                setProvisionStep("form")
                setNewAgentName("")
                setNewAgentDescription("")
                setModalError("")
                setCopiedTarget(null)
              }}
              className="h-9 px-4 text-[10px] font-bold uppercase tracking-[0.18em] shadow-none"
            >
              <Plus size={12} className="mr-1.5" />
              Add Agent Slot
            </Button>
          </div>
        </div>

        <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_400px]">
          <div className="space-y-6 min-w-0">
            <section className="overflow-hidden rounded-md border border-synod-border bg-synod-card">
              <div className="overflow-hidden">
                <table className="w-full table-fixed border-collapse">
                  <thead className="bg-white/[0.02]">
                    <tr className="border-b border-synod-border text-left">
                      <th className="w-[38%] px-4 py-3 text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">Agent</th>
                      <th className="w-[18%] px-3 py-3 text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">Status</th>
                      <th className="w-[20%] px-3 py-3 text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">Last Connected</th>
                      <th className="w-[24%] px-4 py-3 text-right text-[9px] font-bold uppercase tracking-[0.16em] text-synod-muted">Actions</th>
                    </tr>
                  </thead>
                </table>

                <div className={`custom-scrollbar overflow-y-auto ${agents.length > 3 ? "max-h-[196px]" : ""}`}>
                  <table className="w-full table-fixed border-collapse">
                    <tbody>
                      {agents.length === 0 ? (
                        <tr>
                          <td colSpan={4} className="px-6 py-20 text-center">
                            <div className="text-[11px] font-bold uppercase tracking-[0.2em] text-synod-muted">No Agent Slots Yet</div>
                            <p className="mt-3 text-xs text-synod-muted-dark">
                              Create a slot, paste the agent public key, and wallet-sign the binding when you are ready to activate it.
                            </p>
                          </td>
                        </tr>
                      ) : (
                        agents.map((agent) => {
                          const isSelected = selectedAgent?.agent_id === agent.agent_id

                          return (
                            <tr
                              key={agent.agent_id}
                              onClick={() => setSelectedAgentId(agent.agent_id)}
                              className={`cursor-pointer border-b border-synod-border/80 transition-colors last:border-b-0 ${isSelected ? "bg-white/[0.04]" : "hover:bg-white/[0.02]"}`}
                            >
                              <td className="px-4 py-3 align-middle">
                                <div className="truncate text-[11px] font-bold text-white">{agent.name}</div>
                                <div className="mt-0.5 flex items-center gap-3 text-[9px] font-mono text-synod-muted-dark">
                                  <span>{buildAgentToken(agent.agent_id)}</span>
                                  {onManageRules && (
                                    <button
                                      type="button"
                                      onClick={(event) => {
                                        event.stopPropagation()
                                        onManageRules(agent.agent_id)
                                      }}
                                      className="text-synod-warning transition-colors hover:text-white"
                                    >
                                      Manage Rules
                                    </button>
                                  )}
                                </div>
                              </td>
                              <td className="px-3 py-3 align-middle">
                                <span className={`inline-flex items-center rounded-full border px-2 py-1 text-[8px] font-bold uppercase tracking-[0.16em] ${statusClasses(agent.status)}`}>
                                  {displayStatus(agent.status)}
                                </span>
                              </td>
                              <td className="px-3 py-3 align-middle">
                                <div className="font-mono text-[10px] text-synod-muted">{formatRelativeTime(agent.last_connected)}</div>
                              </td>
                              <td className="px-4 py-3 align-middle text-right">
                                <div className="flex justify-end gap-2">
                                  {onManageRules && (
                                    <button
                                      type="button"
                                      onClick={(event) => {
                                        event.stopPropagation()
                                        onManageRules(agent.agent_id)
                                      }}
                                      className="inline-flex items-center justify-center rounded-[5px] border border-synod-border bg-white/[0.03] px-3 py-1.5 text-[9px] font-bold uppercase tracking-[0.14em] text-white transition-colors hover:border-synod-border-strong"
                                    >
                                      Manage Rules
                                    </button>
                                  )}
                                  <button
                                    type="button"
                                    onClick={(event) => {
                                      event.stopPropagation()
                                      setRevokeTarget(agent)
                                      setShowRevokeModal(true)
                                      setModalError("")
                                    }}
                                    disabled={actionLoading === agent.agent_id}
                                    className="inline-flex items-center justify-center rounded-[5px] bg-red-600 px-3 py-1.5 text-[9px] font-bold uppercase tracking-[0.14em] text-white transition-colors hover:bg-red-500 disabled:opacity-60"
                                  >
                                    Revoke
                                  </button>
                                </div>
                              </td>
                            </tr>
                          )
                        })
                      )}
                    </tbody>
                  </table>
                </div>
              </div>
            </section>

            <section className="rounded-md border border-synod-border bg-synod-card">
              <div className="border-b border-synod-border px-5 py-4">
                <div className="text-sm font-bold text-white">SDK Integration Guide</div>
                <div className="mt-1 text-[10px] uppercase tracking-[0.18em] text-synod-muted">For agent developers</div>
              </div>

              <div className="p-5">
                <div className="mb-4 flex flex-wrap gap-2">
                  {(["python", "nodejs", "rust"] as SdkTab[]).map((tab) => (
                    <button
                      key={tab}
                      onClick={() => setSdkTab(tab)}
                      className={`rounded-full border px-3 py-1.5 text-[10px] font-bold uppercase tracking-[0.16em] transition-colors ${sdkTab === tab ? "border-white bg-white text-black" : "border-synod-border bg-white/[0.02] text-synod-muted hover:text-white"}`}
                    >
                      {tab === "nodejs" ? "Node.js" : tab === "python" ? "Python" : "Rust"}
                    </button>
                  ))}
                </div>

                <div className="rounded-md border border-synod-border bg-black px-4 py-4">
                  <div className="mb-3 text-[10px] font-bold uppercase tracking-[0.16em] text-synod-muted-dark">{SDK_SNIPPETS[sdkTab].language}</div>
                  <pre className="custom-scrollbar overflow-x-auto font-mono text-[11px] leading-6">
                    {SDK_SNIPPETS[sdkTab].lines.map((line, index) => (
                      <div key={`${sdkTab}-${index}`} className={snippetToneClass(line.tone)}>
                        {line.text || " "}
                      </div>
                    ))}
                  </pre>
                </div>
              </div>
            </section>
          </div>

          <aside className="rounded-md border border-synod-border bg-synod-card">
            {selectedAgent ? (
              <>
                <div className="flex items-center justify-between border-b border-synod-border px-5 py-4">
                  <div>
                    <div className="text-sm font-bold text-white">{selectedAgent.name}</div>
                    <div className="mt-1 text-[10px] uppercase tracking-[0.18em] text-synod-muted">Approved agent signer slot</div>
                  </div>
                  <span className={`inline-flex items-center rounded-full border px-2.5 py-1 text-[9px] font-bold uppercase tracking-[0.18em] ${statusClasses(selectedAgent.status)}`}>
                    {displayStatus(selectedAgent.status)}
                  </span>
                </div>

                <div className="space-y-6 p-5">
                  <section className="space-y-2">
                    <div className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted">Agent ID</div>
                    <div className="rounded-md border border-synod-border bg-black px-4 py-3">
                      <div className="flex items-center justify-between gap-3">
                        <div className="font-mono text-[11px] text-white break-all">{selectedAgent.agent_id}</div>
                        <button
                          type="button"
                          onClick={() => copyToClipboard(selectedAgent.agent_id, "agent_id")}
                          className="inline-flex items-center rounded-md border border-synod-border bg-white/[0.03] px-3 py-2 text-[10px] font-bold uppercase tracking-[0.16em] text-white transition-colors hover:border-synod-border-strong"
                        >
                          <Copy size={12} className="mr-1.5" />
                          {copiedTarget === "agent_id" ? "Copied" : "Copy"}
                        </button>
                      </div>
                    </div>
                  </section>

                  <section className="space-y-2">
                    <div className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted">Description</div>
                    <div className="rounded-md border border-synod-border bg-black px-4 py-3 text-sm text-white">
                      {selectedAgent.description?.trim() || "No description provided for this slot."}
                    </div>
                  </section>

                  <section className="space-y-3">
                    <div className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted">Bind Public Key</div>
                    <div className="space-y-3 rounded-md border border-synod-border bg-black px-4 py-4">
                      <p className="text-sm leading-6 text-synod-muted">
                        Run your agent or `synod-mcp`, copy its public key, paste it here, then sign the binding with your treasury wallet.
                      </p>

                      <div className="space-y-2">
                        <label className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Agent Public Key</label>
                        <textarea
                          value={enrollmentPubkey}
                          onChange={(event) => setEnrollmentPubkey(event.target.value.trim())}
                          rows={3}
                          placeholder="G..."
                          className="w-full rounded-xl border border-synod-border bg-[#050508] px-4 py-3 font-mono text-[11px] text-white outline-none transition-colors placeholder:text-synod-muted-dark focus:border-white"
                        />
                      </div>

                      {selectedAgent.agent_pubkey && (
                        <div className="rounded-xl border border-emerald-500/20 bg-emerald-500/10 px-4 py-3">
                          <div className="flex items-center justify-between gap-3">
                            <div className="min-w-0">
                              <div className="text-[9px] uppercase tracking-[0.16em] text-emerald-200/80">Enrolled Key</div>
                              <div className="mt-1 break-all font-mono text-[11px] text-white">{selectedAgent.agent_pubkey}</div>
                            </div>
                            <button
                              type="button"
                              onClick={() => copyToClipboard(selectedAgent.agent_pubkey!, "agent_pubkey")}
                              className="inline-flex items-center rounded-md border border-emerald-400/20 bg-white/[0.03] px-3 py-2 text-[10px] font-bold uppercase tracking-[0.16em] text-white transition-colors hover:border-emerald-300/40"
                            >
                              <Copy size={12} className="mr-1.5" />
                              {copiedTarget === "agent_pubkey" ? "Copied" : "Copy"}
                            </button>
                          </div>
                        </div>
                      )}

                      {bindingError && <div className="rounded-xl border border-red-500/25 bg-red-500/10 px-4 py-3 text-xs text-red-200">{bindingError}</div>}
                      {bindingStatus && !bindingError && (
                        <div className="rounded-xl border border-sky-500/20 bg-sky-500/10 px-4 py-3 text-xs text-sky-100">
                          {bindingStatus}
                        </div>
                      )}

                      <div className="flex justify-end">
                        <Button
                          type="button"
                          variant="primary"
                          size="sm"
                          onClick={handleBindPublicKey}
                          disabled={!enrollmentPubkey.trim() || bindingAgentId === selectedAgent.agent_id}
                          className="h-10 px-4 text-[10px] font-bold uppercase tracking-[0.16em]"
                        >
                          {bindingAgentId === selectedAgent.agent_id ? "Binding..." : selectedAgent.agent_pubkey ? "Rebind Key" : "Bind Key"}
                        </Button>
                      </div>
                    </div>
                  </section>

                  <section className="space-y-3">
                    <div className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted">Capital Rules</div>
                    <div className="rounded-md border border-synod-border bg-black px-4 py-4 space-y-3">
                      <p className="text-sm leading-6 text-synod-muted">
                        This page only manages slot identity and signer enrollment. Wallet access, allocation, tier limits, and concurrency rules live in Policy.
                      </p>
                      {onManageRules && (
                        <Button
                          type="button"
                          variant="secondary"
                          size="sm"
                          onClick={() => onManageRules(selectedAgent.agent_id)}
                          className="h-10 border border-synod-border px-4 text-[10px] font-bold uppercase tracking-[0.16em]"
                        >
                          Manage Rules
                        </Button>
                      )}
                    </div>
                  </section>

                  <section className="space-y-3">
                    <div className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted">Connection</div>
                    <div className="space-y-3 rounded-md border border-synod-border bg-black px-4 py-4">
                      <div>
                        <div className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Last Connected</div>
                        <div className="mt-1 font-mono text-[11px] text-white">{formatDate(selectedAgent.last_connected)}</div>
                      </div>

                      <div>
                        <div className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Created At</div>
                        <div className="mt-1 font-mono text-[11px] text-white">{formatDate(selectedAgent.created_at)}</div>
                      </div>

                      <div>
                        <div className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Agent Pubkey</div>
                        <div className="mt-1 font-mono text-[11px] text-white break-all">
                          {selectedAgent.agent_pubkey ? selectedAgent.agent_pubkey : "Not registered yet"}
                        </div>
                      </div>

                      <div>
                        <div className="text-[9px] uppercase tracking-[0.16em] text-synod-muted-dark">Activation Flow</div>
                        <div className="mt-1 text-[11px] leading-6 text-synod-muted">
                          Bind the public key, configure capital rules, make the key an on-chain signer, then let the agent complete the Synod Connect challenge.
                        </div>
                      </div>
                    </div>
                  </section>
                </div>
              </>
            ) : (
              <div className="flex h-full min-h-[420px] items-center justify-center p-8 text-center">
                <div>
                  <div className="text-[11px] font-bold uppercase tracking-[0.2em] text-synod-muted">No Agent Selected</div>
                  <p className="mt-3 text-xs text-synod-muted-dark">Create a slot or select an existing row to inspect its identity and connection state.</p>
                </div>
              </div>
            )}
          </aside>
        </div>
      </div>

      {showProvisionModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6 backdrop-blur-sm">
          <div className="w-full max-w-[560px] rounded-2xl border border-synod-border bg-[#07070b] shadow-2xl">
            {provisionStep === "form" ? (
              <form onSubmit={handleProvision}>
                <div className="flex items-center justify-between border-b border-synod-border px-6 py-5">
                  <div className="text-2xl font-bold text-white tracking-tight">Create Agent</div>
                  <button type="button" onClick={resetProvisionModal} className="text-synod-muted transition-colors hover:text-white">
                    <X size={20} />
                  </button>
                </div>

                <div className="space-y-6 px-6 py-6">
                  <p className="max-w-xl text-sm leading-7 text-synod-muted">
                    Agent slots create identity and credentials only. Capital rules are configured later in Policy.
                  </p>

                  <div className="space-y-2">
                    <label className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted-dark">Agent Name</label>
                    <input
                      autoFocus
                      required
                      maxLength={64}
                      type="text"
                      value={newAgentName}
                      onChange={(event) => setNewAgentName(event.target.value)}
                      placeholder="e.g. Yield Optimizer Bot"
                      className="h-14 w-full rounded-xl border border-synod-border bg-black px-4 font-mono text-sm text-white outline-none transition-colors placeholder:text-synod-muted-dark focus:border-white"
                    />
                    <div className="text-[10px] text-synod-muted-dark">Required. Maximum 64 characters.</div>
                  </div>

                  <div className="space-y-2">
                    <label className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted-dark">Description</label>
                    <textarea
                      maxLength={255}
                      value={newAgentDescription}
                      onChange={(event) => setNewAgentDescription(event.target.value)}
                      placeholder="Optional context for operators and reviewers"
                      className="min-h-[120px] w-full rounded-xl border border-synod-border bg-black px-4 py-4 text-sm text-white outline-none transition-colors placeholder:text-synod-muted-dark focus:border-white"
                    />
                    <div className="text-[10px] text-synod-muted-dark">Optional. Maximum 255 characters.</div>
                  </div>

                  {modalError && <div className="rounded-xl border border-red-500/25 bg-red-500/10 px-4 py-3 text-xs text-red-200">{modalError}</div>}
                </div>

                <div className="flex justify-end px-6 pb-6">
                  <button
                    type="submit"
                    disabled={isProvisioning}
                    className="inline-flex h-12 items-center justify-center rounded-lg bg-white px-6 text-[11px] font-bold uppercase tracking-[0.16em] text-black transition-colors hover:bg-zinc-200 disabled:opacity-60"
                  >
                    {isProvisioning ? "Creating..." : "Create"}
                  </button>
                </div>
              </form>
            ) : provisionResult ? (
              <div>
                <div className="flex items-center justify-between border-b border-synod-border px-6 py-5">
                  <div className="text-2xl font-bold text-white tracking-tight">Agent Created</div>
                  <button type="button" onClick={resetProvisionModal} className="text-synod-muted transition-colors hover:text-white">
                    <X size={20} />
                  </button>
                </div>

                <div className="space-y-6 px-6 py-6">
                  <div className="flex items-start gap-3 rounded-xl border border-emerald-500/20 bg-emerald-500/10 px-4 py-4">
                    <CheckCircle2 size={18} className="mt-0.5 shrink-0 text-emerald-300" />
                    <div>
                      <div className="text-sm font-bold text-white">{provisionResult.agent.name}</div>
                      <p className="mt-2 text-sm leading-6 text-synod-muted">
                        The slot is ready. Next, run your agent, copy its generated public key, and bind it from this dashboard before the agent calls `connect()`.
                      </p>
                    </div>
                  </div>

                  <section className="space-y-2">
                    <label className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted-dark">Agent ID</label>
                    <div className="rounded-xl border border-synod-border bg-black px-4 py-4">
                      <div className="flex items-center justify-between gap-3">
                        <div className="min-h-6 font-mono text-sm text-white break-all">{provisionResult.agent.agent_id}</div>
                        <button
                          type="button"
                          onClick={() => copyToClipboard(provisionResult.agent.agent_id, "agent_id")}
                          className="inline-flex items-center rounded-md border border-synod-border bg-white/[0.03] px-3 py-2 text-[10px] font-bold uppercase tracking-[0.16em] text-white transition-colors hover:border-synod-border-strong"
                        >
                          <Copy size={12} className="mr-1.5" />
                          {copiedTarget === "agent_id" ? "Copied" : "Copy"}
                        </button>
                      </div>
                    </div>
                  </section>

                  <section className="space-y-2">
                    <label className="text-[10px] font-bold uppercase tracking-[0.18em] text-synod-muted-dark">What To Do Next</label>
                    <div className="rounded-xl border border-synod-border bg-black px-4 py-4 text-sm leading-7 text-synod-muted">
                      1. Start your agent or `synod-mcp` so it generates a local keypair.
                      <br />
                      2. Copy the printed public key into this slot.
                      <br />
                      3. Sign the binding with your wallet.
                      <br />
                      4. Let the agent finish Synod Connect and open its WebSocket session.
                    </div>
                  </section>
                </div>

                <div className="flex justify-end px-6 pb-6">
                  <button
                    type="button"
                    onClick={resetProvisionModal}
                    className="inline-flex h-12 items-center justify-center rounded-lg bg-white px-6 text-[11px] font-bold uppercase tracking-[0.16em] text-black transition-colors hover:bg-zinc-200"
                  >
                    Done
                  </button>
                </div>
              </div>
            ) : null}
          </div>
        </div>
      )}

      {showRevokeModal && revokeTarget && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-2xl border border-synod-border bg-[#07070b] shadow-2xl">
            <div className="flex items-center justify-between border-b border-synod-border px-6 py-5">
              <div className="text-xl font-bold text-white">Confirm Revocation</div>
              <button
                type="button"
                onClick={() => {
                  setShowRevokeModal(false)
                  setRevokeTarget(null)
                  setModalError("")
                }}
                className="text-synod-muted transition-colors hover:text-white"
              >
                <X size={20} />
              </button>
            </div>

            <div className="space-y-5 px-6 py-6">
              <div className="flex items-start gap-3 rounded-xl border border-red-500/20 bg-red-500/10 px-4 py-4">
                <AlertTriangle size={18} className="mt-0.5 shrink-0 text-red-300" />
                <div>
                  <div className="text-sm font-bold text-white">{revokeTarget.name}</div>
                  <p className="mt-2 text-sm leading-6 text-synod-muted">
                    Revoking this slot disconnects the agent, invalidates its runtime session, and removes the enrolled signer identity. Use this only when the slot should be retired.
                  </p>
                </div>
              </div>

              {modalError && <div className="rounded-xl border border-red-500/25 bg-red-500/10 px-4 py-3 text-xs text-red-200">{modalError}</div>}
            </div>

            <div className="flex justify-end gap-3 px-6 pb-6">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => {
                  setShowRevokeModal(false)
                  setRevokeTarget(null)
                  setModalError("")
                }}
                className="h-10 border border-synod-border px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-synod-muted hover:text-white"
              >
                Cancel
              </Button>
              <button
                type="button"
                onClick={handleRevoke}
                disabled={actionLoading === revokeTarget.agent_id}
                className="inline-flex h-10 items-center justify-center rounded-lg bg-red-600 px-4 text-[10px] font-bold uppercase tracking-[0.16em] text-white transition-colors hover:bg-red-500 disabled:opacity-60"
              >
                {actionLoading === revokeTarget.agent_id ? "Revoking..." : "Confirm Revoke"}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}
