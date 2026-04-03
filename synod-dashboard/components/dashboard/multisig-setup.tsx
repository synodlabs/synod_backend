"use client"

import { useState, useEffect } from "react"
import { Shield, Lock, CheckCircle2, AlertCircle, Loader2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useStellarWallet } from "@/hooks/use-stellar-wallet"
import { 
    StellarWalletsKit,
} from '@creit.tech/stellar-wallets-kit';

interface MultisigSetupProps {
  treasuryId: string
  onStatusChange?: (isActive: boolean) => void
}

export function MultisigSetup({ treasuryId, onStatusChange }: MultisigSetupProps) {
  const { address } = useStellarWallet()
  const [loading, setLoading] = useState(false)
  const [isActive, setIsActive] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [coordinatorPubkey, setCoordinatorPubkey] = useState<string>("")

  useEffect(() => {
    // Check initial status
    const checkStatus = async () => {
        try {
            const token = localStorage.getItem('synod_token');
            const res = await fetch(`/v1/dashboard/${treasuryId}`, {
                headers: { 'Authorization': `Bearer ${token}` }
            });
            if (res.ok) {
                const data = await res.json();
                // Check if any wallet has multisig active
                const hasMultisig = data.wallets?.some((w: any) => w.multisig_active);
                setIsActive(hasMultisig);
            }
        } catch (e) {
            console.error("Status check failed", e);
        }
    };
    checkStatus();
  }, [treasuryId]);

  const handleSetup = async () => {
    setLoading(true)
    setError(null)

    try {
      const token = localStorage.getItem('synod_token');
      
      // 1. Get Setup XDR
      const setupRes = await fetch(`/v1/multisig/${treasuryId}/setup`, {
         headers: { 'Authorization': `Bearer ${token}` }
      });
      
      if (!setupRes.ok) throw new Error("Failed to generate multisig setup");
      const { xdr, coordinator_pubkey } = await setupRes.json();
      setCoordinatorPubkey(coordinator_pubkey);

      // 2. Sign and Submit via Wallet Kit
      // Note: StellarWalletsKit.signTransaction returns the signed XDR 
      // but some modules (like Freighter) can also submit.
      // For simplicity in Phase 3, we'll use signAndSubmitTransaction if available, 
      // or sign and manually submit.
      
      try {
          const result = await StellarWalletsKit.signTransaction(xdr);
          console.log("Signed Transaction XDR:", result.signedTxXdr);
          
          // In a real flow, we'd submit to Horizon here. 
          // For now, let's assume the user's wallet handled submission (if using Freighter) 
          // or we provide a "Confirm" step.
          
          // 3. Confirm with Backend
          const confirmRes = await fetch(`/v1/multisig/${treasuryId}/confirm`, {
              method: 'POST',
              headers: { 
                  'Authorization': `Bearer ${token}`,
                  'Content-Type': 'application/json'
              }
          });
          
          if (confirmRes.ok) {
              setIsActive(true);
              if (onStatusChange) onStatusChange(true);
          } else {
              throw new Error("Failed to confirm multisig on-chain");
          }

      } catch (err: any) {
          console.error("Signing/Submission error", err);
          setError(err.message || "Signing rejected or failed");
      }

    } catch (err: any) {
      setError(err.message || "Something went wrong")
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="bg-synod-card border border-synod-border p-8 rounded-md">
      <div className="flex items-start gap-4">
        <div className={`p-3 rounded-sm border ${isActive ? 'bg-white border-white' : 'bg-white/5 border-white/10'}`}>
          {isActive ? (
            <Shield className="w-6 h-6 text-black" />
          ) : (
            <Lock className="w-6 h-6 text-white" />
          )}
        </div>
        
        <div className="flex-1">
          <div className="flex items-center gap-3 mb-1">
            <h3 className="text-sm font-bold text-white tracking-tight uppercase">
              Security Architecture
            </h3>
            {isActive ? (
                <span className="px-2 py-0.5 rounded-sm bg-white text-black text-[9px] font-bold uppercase tracking-widest">
                    ACTIVE
                </span>
            ) : (
                <span className="px-2 py-0.5 rounded-sm border border-white/20 text-white text-[9px] font-bold uppercase tracking-widest">
                    UNSECURED
                </span>
            )}
          </div>
          
          <p className="text-[11px] text-synod-muted mb-6 max-w-md leading-relaxed">
            {isActive 
              ? "Dual-key protocol enforced. All outbound transactions require multi-party cryptographic consensus."
              : "Establish dual-key security to prevent unauthorized capital movement. Coordination layer signature will be required."
            }
          </p>

          {!isActive && (
            <div className="space-y-4">
              <div className="p-4 rounded-sm bg-white/5 border border-white/5 flex items-start gap-3">
                <AlertCircle className="w-4 h-4 text-synod-muted-dark shrink-0 mt-0.5" />
                <div className="text-[11px] text-synod-muted leading-relaxed">
                  Requires <code className="text-white">SetOptions</code> signature. 
                  Adds Coordinator co-signer (+0.5 XLM reserve).
                </div>
              </div>

              <button
                onClick={handleSetup}
                disabled={loading || !address}
                className="w-full h-12 bg-white text-black font-bold uppercase text-[10px] tracking-widest hover:bg-zinc-200 transition-all disabled:opacity-50"
              >
                {loading ? (
                  <Loader2 className="w-4 h-4 animate-spin" />
                ) : (
                  "ESTABLISH DUAL-KEY SECURITY"
                )}
              </button>
            </div>
          )}

          {error && (
            <div className="mt-4 text-[10px] text-red-400 font-bold uppercase bg-red-400/5 p-3 rounded-sm border border-red-400/20">
                ERROR: {error}
            </div>
          )}

          {isActive && (
            <div className="p-4 rounded-sm bg-white/5 border border-white/10 flex items-center gap-3">
              <CheckCircle2 className="w-4 h-4 text-white" />
              <div className="text-[10px] text-white font-bold uppercase tracking-widest">
                Co-signer weight established.
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
