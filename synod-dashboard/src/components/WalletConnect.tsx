import { useState } from 'react';
import { Wallet, Plus } from 'lucide-react';

interface WalletConnectProps {
  treasuryId: string;
  token: string;
  onSuccess: () => void;
}

export default function WalletConnect({ treasuryId, token, onSuccess }: WalletConnectProps) {
  const [address, setAddress] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [nonce, setNonce] = useState<string | null>(null);

  const handleRegister = async () => {
    setLoading(true);
    setError('');
    try {
      // 1. Get Nonce
      const nRes = await fetch('/v1/wallets/nonce', {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      const nData = await nRes.json();
      setNonce(nData.nonce);

      // 2. Register Wallet
      const res = await fetch(`/v1/treasuries/${treasuryId}/wallets`, {
        method: 'POST',
        headers: { 
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}` 
        },
        body: JSON.stringify({ wallet_address: address, label: 'Main Savings' }),
      });

      if (!res.ok) throw new Error('Failed to register wallet');
      
      setNonce(null);
      onSuccess();
    } catch (err: any) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="p-6 rounded-2xl bg-white/5 border border-white/10">
      <div className="flex items-center gap-3 mb-6">
        <Wallet className="text-synod-accent w-6 h-6" />
        <h2 className="text-xl font-bold text-white">Connect Stellar Wallet</h2>
      </div>

      <div className="space-y-4">
        <div>
          <label className="block text-gray-400 text-xs uppercase tracking-widest font-semibold mb-2">Wallet Address (G...)</label>
          <input
            type="text"
            className="w-full bg-black/40 border border-white/10 rounded-lg py-3 px-4 text-sm text-white focus:outline-none focus:border-synod-accent"
            placeholder="GA..."
            value={address}
            onChange={e => setAddress(e.target.value)}
          />
        </div>

        {error && <div className="text-synod-error text-xs">{error}</div>}

        <button
          onClick={handleRegister}
          disabled={loading || !address}
          className="w-full h-12 flex items-center justify-center gap-2 bg-white/10 hover:bg-white/20 text-white rounded-lg transition-colors border border-white/5"
        >
          <Plus size={18} />
          {loading ? 'Connecting...' : 'Register Wallet'}
        </button>
      </div>

      {nonce && (
        <div className="mt-4 p-3 bg-synod-accent/10 border border-synod-accent/20 rounded-lg">
          <p className="text-xs text-synod-accent text-center">
            Sign this nonce to verify ownership:<br/>
            <span className="font-mono font-bold break-all">{nonce}</span>
          </p>
        </div>
      )}
    </div>
  );
}
