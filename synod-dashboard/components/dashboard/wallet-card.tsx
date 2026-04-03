"use client"

import { useState, useEffect } from "react"
import { Shield, MoreHorizontal, CheckCircle2, Loader2, AlertCircle, X, ExternalLink, AlertTriangle } from "lucide-react"
import { Horizon, TransactionBuilder, Networks, Operation } from '@stellar/stellar-sdk'
import { StellarWalletsKit } from '@creit.tech/stellar-wallets-kit'
import { Button } from "@/components/ui/button"
import { useStellarWallet } from "@/hooks/use-stellar-wallet"

interface WalletCardProps {
  treasuryId: string;
  token: string | null;
  wallet: {
    wallet_address: string;
    label: string | null;
    multisig_active: boolean;
    status: string;
  };
  onDisconnect?: (address: string) => void;
  onBalanceUpdate?: (address: string, aum: number) => void;
}

interface Balance {
  asset_code: string;
  balance: string;
  usd_value: number;
}

export function WalletCard({ treasuryId, token, wallet, onDisconnect, onBalanceUpdate }: WalletCardProps) {
  const { address: activeAddress } = useStellarWallet()
  const [balances, setBalances] = useState<Balance[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")
  const [isDisconnecting, setIsDisconnecting] = useState(false)
  const [showConfirm, setShowConfirm] = useState(false)
  const [revokeStep, setRevokeStep] = useState<'idle' | 'preparing' | 'signing' | 'submitting' | 'backend'>('idle')
  const [revokeStatus, setRevokeStatus] = useState("")

  const server = new Horizon.Server("https://horizon-testnet.stellar.org")

  useEffect(() => {
    async function fetchBalances() {
      setLoading(true)
      try {
        const account = await server.loadAccount(wallet.wallet_address)
        const newBalances: Balance[] = account.balances.map((b: any) => {
          const code = b.asset_type === "native" ? "XLM" : b.asset_code;
          // Mock USD values for Phase 3 (USDC is 1:1, XLM is ~$0.15)
          const amount = parseFloat(b.balance);
          const usdValue = code === "USDC" ? amount : (code === "XLM" ? amount * 0.15 : 0);

          return {
            asset_code: code,
            balance: amount.toLocaleString(undefined, { minimumFractionDigits: 2 }),
            usd_value: usdValue
          }
        })

        setBalances(newBalances)
        const totalAum = newBalances.reduce((sum, b) => sum + b.usd_value, 0)
        onBalanceUpdate?.(wallet.wallet_address, totalAum)
      } catch (err) {
        console.error("Failed to fetch balance", err)
        setError("Network error")
      } finally {
        setLoading(false)
      }
    }

    fetchBalances()
    // Refresh every 30 seconds
    const interval = setInterval(fetchBalances, 30000)
    return () => clearInterval(interval)
  }, [wallet.wallet_address])

  const handleSecureDisconnect = async () => {
    if (!token) return
    setIsDisconnecting(true)
    setError("")
    try {
      // 1. Fetch coordinator key
      setRevokeStep('preparing')
      setRevokeStatus("Fetching security context...")
      const setupRes = await fetch(`/v1/multisig/${treasuryId}/setup`, {
        headers: { "Authorization": `Bearer ${token}` }
      })
      if (!setupRes.ok) throw new Error("Could not fetch co-signer info")
      const { coordinator_pubkey } = await setupRes.json()

      // 2. Build Revocation TX
      setRevokeStatus("Building revocation transaction...")
      const account = await server.loadAccount(wallet.wallet_address)
      const tx = new TransactionBuilder(account, {
        fee: "1000",
        networkPassphrase: Networks.TESTNET
      })
        .addOperation(Operation.setOptions({
          signer: {
            ed25519PublicKey: coordinator_pubkey,
            weight: 0 // Zero weight removes the signer
          },
          lowThreshold: 0,
          medThreshold: 0,
          highThreshold: 0
        }))
        .setTimeout(30)
        .build()

      // 3. Verify Active Wallet matches Card Wallet
      setRevokeStep('signing')
      setRevokeStatus("Verifying session...")
      const { address: currentActive } = await StellarWalletsKit.fetchAddress()
      if (currentActive !== wallet.wallet_address) {
        throw new Error(`Account Mismatch: Your wallet extension is set to ${currentActive.substring(0, 8)}... Please switch to ${wallet.wallet_address.substring(0, 8)}...`)
      }

      // ── SELF-HEALING: Inspect on-chain state to decide submission strategy ──
      const onChainAccount = await server.loadAccount(wallet.wallet_address)
      const isCoordSigner = onChainAccount.signers.some(
        (s: any) => s.key === coordinator_pubkey && s.weight > 0
      )
      // Thresholds: SetOptions (changing signers) is a HIGH threshold operation.
      // If high_threshold is 0, Stellar needs only 1 valid signature.
      // If high_threshold > user's master weight, we NEED the coordinator co-signature.
      const highThreshold = onChainAccount.thresholds?.high_threshold ?? 0
      const masterWeight = onChainAccount.signers.find(
        (s: any) => s.key === wallet.wallet_address
      )?.weight ?? 1

      const needsCosign = isCoordSigner && highThreshold > masterWeight

      console.log(`[Revoke] On-chain state: coordinator_is_signer=${isCoordSigner}, high_threshold=${highThreshold}, master_weight=${masterWeight}, needs_cosign=${needsCosign}`)

      setRevokeStatus("Sign revocation in your wallet...")
      const result = await StellarWalletsKit.signTransaction(tx.toXDR())
      if (!result) throw new Error("Revocation signing rejected")

      if (!needsCosign) {
        // ── DIRECT SUBMISSION: Thresholds allow single-sig ──
        console.log("[Revoke] Direct submission (single-sig sufficient)")
        setRevokeStep('submitting')
        setRevokeStatus("Submitting directly to Stellar...")
        const signedTxDirect = TransactionBuilder.fromXDR(result.signedTxXdr, Networks.TESTNET)
        await server.submitTransaction(signedTxDirect as any)

        // Cleanup backend records
        await fetch(`/v1/multisig/${treasuryId}/revoke`, {
          method: "POST",
          headers: {
            "Authorization": `Bearer ${token}`,
            "Content-Type": "application/json"
          },
          body: JSON.stringify({
            xdr: "OFF_CHAIN_BYPASS",
            wallet_address: wallet.wallet_address
          })
        })
        // Disconnect wallet extension session
        try { StellarWalletsKit.disconnect() } catch (_) { }
        onDisconnect?.(wallet.wallet_address)
      } else {
        // ── CO-SIGNED SUBMISSION: Thresholds require both signatures ──
        console.log("[Revoke] Co-sign submission (2-of-2 required)")
        setRevokeStep('submitting')
        setRevokeStatus("Finalizing revocation with Synod Security...")
        const res = await fetch(`/v1/multisig/${treasuryId}/revoke`, {
          method: "POST",
          headers: {
            "Authorization": `Bearer ${token}`,
            "Content-Type": "application/json"
          },
          body: JSON.stringify({
            xdr: result.signedTxXdr,
            wallet_address: wallet.wallet_address
          })
        })

        if (res.ok) {
          // Disconnect wallet extension session
          try { StellarWalletsKit.disconnect() } catch (_) { }
          onDisconnect?.(wallet.wallet_address)
        } else {
          const errData = await res.json().catch(() => ({}));
          console.error("Detailed Stellar Error:", errData);
          throw new Error(errData.message?.split('{')[0] || "Revocation failed. Please try again.")
        }
      }

    } catch (err: any) {
      console.error("Revocation failed:", err)
      setError(err.message || "Revocation failed")
      setRevokeStep('idle')
    } finally {
      setIsDisconnecting(false)
      setRevokeStatus("")
    }
  }

  const totalAum = balances.reduce((sum, b) => sum + b.usd_value, 0)

  return (
    <div className="bg-synod-card border border-synod-border rounded-md overflow-hidden flex flex-col group hover:border-white/20 transition-all">
      {/* Header */}
      <div className="px-6 py-4 border-b border-synod-border bg-gradient-to-br from-white/[0.02] to-transparent">
        <div className="flex justify-between items-start mb-2">
          <div className="w-10 h-10 bg-white/5 border border-white/10 rounded flex items-center justify-center">
            <Shield className="w-5 h-5 text-white" />
          </div>
          <div className="flex items-center gap-2">
            <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-emerald-500/10 border border-emerald-500/20 text-[9px] font-bold text-emerald-400 uppercase tracking-widest">
              <div className="w-1 h-1 bg-emerald-400 rounded-full animate-pulse" />
              {wallet.status === 'ACTIVE' ? 'ACTIVE' : 'SYNCING'}
            </span>
            <button className="text-synod-muted-dark hover:text-white transition-colors">
              <MoreHorizontal size={16} />
            </button>
          </div>
        </div>

        <div>
          <h3 className="text-sm font-bold text-white tracking-tight">{wallet.label || "Unnamed Wallet"}</h3>
          <p className="text-[10px] text-synod-muted-dark font-mono mt-1 flex items-center gap-1.5">
            {wallet.wallet_address.substring(0, 12)}...{wallet.wallet_address.substring(44)}
            <ExternalLink size={10} className="inline opacity-50 group-hover:opacity-100 cursor-pointer" />
          </p>
        </div>
      </div>

      {/* Assets */}
      <div className="px-6 py-4 flex-1 space-y-3">
        {loading && balances.length === 0 ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="w-5 h-5 animate-spin text-synod-muted-dark" />
          </div>
        ) : error ? (
          <div className="text-[10px] text-red-400 font-bold uppercase py-8 text-center">{error}</div>
        ) : (
          balances.map(b => (
            <div key={b.asset_code} className="flex justify-between items-end">
              <div className="space-y-1">
                <div className="flex items-center gap-2">
                  <div className="w-5 h-5 rounded-full bg-white/5 border border-white/10 flex items-center justify-center text-[8px] font-bold">
                    {b.asset_code.charAt(0)}
                  </div>
                  <span className="text-xs font-bold text-white">{b.asset_code}</span>
                </div>
                <div className="text-[9px] text-synod-muted-dark uppercase tracking-wider pl-7 font-mono">
                  {b.asset_code === 'XLM' ? 'native' : 'circle.io'}
                </div>
              </div>
              <div className="text-right">
                <div className="text-[13px] font-bold text-white font-mono">{b.balance}</div>
                <div className="text-[10px] text-synod-muted-dark font-mono mt-0.5">≈ ${b.usd_value.toLocaleString()}</div>
              </div>
            </div>
          ))
        )}
      </div>

      {/* Footer Metrics */}
      <div className="grid grid-cols-2 border-t border-synod-border divide-x divide-synod-border">
        <div className="p-4 space-y-1">
          <div className="text-[9px] text-synod-muted uppercase tracking-widest font-bold">Pools</div>
          <div className="text-[10px] text-white font-mono truncate">trading, ops_reserve</div>
        </div>
        <div className="p-4 space-y-1">
          <div className="text-[9px] text-synod-muted uppercase tracking-widest font-bold">AUM</div>
          <div className="text-[10px] text-white font-mono">${totalAum.toLocaleString()}</div>
        </div>
      </div>

      {/* Action Bar */}
      <div className="px-6 py-3 bg-black/40 flex justify-between items-center bg-gradient-to-t from-white/[0.01] to-transparent">
        <div className="flex items-center gap-2">
          {wallet.multisig_active ? (
            <>
              <Shield className="w-3.5 h-3.5 text-white/40" />
              <span className="text-[9px] font-bold text-synod-muted-dark uppercase tracking-[0.1em]">2-of-2 multisig active</span>
            </>
          ) : (
            <>
              <AlertCircle className="w-3.5 h-3.5 text-zinc-600" />
              <span className="text-[9px] font-bold text-synod-muted-dark uppercase tracking-[0.1em]">multisig pending</span>
            </>
          )}
        </div>
        <div className="flex gap-2">
          <button className="px-3 py-1.5 rounded-sm bg-white/5 border border-white/5 text-[9px] font-bold text-white uppercase tracking-widest hover:bg-white/10 transition-colors">
            Details
          </button>
          <button
            onClick={() => setShowConfirm(true)}
            disabled={isDisconnecting}
            className="px-3 py-1.5 rounded-sm bg-red-400/5 border border-red-400/10 text-[9px] font-bold text-red-400/80 uppercase tracking-widest hover:bg-red-400/10 transition-colors flex items-center gap-2"
          >
            {isDisconnecting ? <Loader2 className="w-3 h-3 animate-spin" /> : "Disconnect"}
          </button>
        </div>
      </div>

      {/* Revocation Confirmation Modal */}
      {showConfirm && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center p-6 bg-black/90 backdrop-blur-md animate-in fade-in duration-300">
          <div className="bg-synod-card border border-synod-border w-full max-w-sm p-8 rounded-md relative shadow-2xl">
            <div className="flex flex-col items-center text-center space-y-6">
              <div className="w-16 h-16 bg-red-500/10 border border-red-500/20 rounded-full flex items-center justify-center">
                <AlertTriangle className="w-8 h-8 text-red-400" />
              </div>

              <div className="space-y-2">
                <h4 className="text-sm font-bold text-white uppercase tracking-widest">Confirm Revocation</h4>
                <p className="text-[11px] text-synod-muted leading-relaxed">
                  This will permanently remove the Synod Coordinator as a co-signer on the network and unlink the wallet from this treasury.
                </p>
              </div>

              {error && (
                <div className="w-full p-3 bg-red-400/5 border border-red-400/10 rounded-sm text-[10px] text-red-400 font-bold uppercase overflow-hidden text-ellipsis">
                  {error.length > 100 ? error.substring(0, 100) + "..." : error}
                </div>
              )}

              <div className="flex flex-col w-full gap-3">
                <button
                  onClick={handleSecureDisconnect}
                  disabled={isDisconnecting}
                  className="w-full h-12 bg-red-500 text-white font-bold uppercase text-[10px] tracking-widest hover:bg-red-600 transition-all flex items-center justify-center gap-2"
                >
                  {isDisconnecting ? (
                    <>
                      <Loader2 className="w-4 h-4 animate-spin" />
                      {revokeStatus || "PROCESSING..."}
                    </>
                  ) : "SIGN & REVOKE ACCESS"}
                </button>
                <button
                  onClick={() => !isDisconnecting && setShowConfirm(false)}
                  disabled={isDisconnecting}
                  className="w-full h-12 bg-white/5 border border-white/10 text-white font-bold uppercase text-[10px] tracking-widest hover:bg-white/10 transition-all"
                >
                  CANCEL
                </button>
              </div>

              <p className="text-[9px] text-synod-muted-dark font-mono uppercase tracking-tighter">
                Dual-Signature Removal will trigger a network transaction.
              </p>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
