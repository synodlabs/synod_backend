import { useState } from 'react';
import { Cpu, Key, Terminal } from 'lucide-react';

interface AgentManagerProps {
  treasuryId: string;
  token: string;
}

export default function AgentManager({ treasuryId, token }: AgentManagerProps) {
  const [loading, setLoading] = useState(false);
  const [newAgent, setNewAgent] = useState<{ id: string, key: string } | null>(null);

  const createAgent = async () => {
    setLoading(true);
    try {
      const res = await fetch('/v1/agents', {
        method: 'POST',
        headers: { 
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}` 
        },
        body: JSON.stringify({ treasury_id: treasuryId, name: 'New Strategy Agent' }),
      });
      const data = await res.json();
      setNewAgent({ id: data.agent_id, key: data.api_key });
    } catch (err) {
      console.error(err);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="p-6 rounded-2xl bg-white/5 border border-white/10 mt-6">
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-3">
          <Cpu className="text-synod-accent w-6 h-6" />
          <h2 className="text-xl font-bold text-white">Agent Slots</h2>
        </div>
        <button 
          onClick={createAgent}
          disabled={loading}
          className="bg-synod-accent/10 border border-synod-accent/20 text-synod-accent px-4 py-2 rounded-lg text-sm font-bold hover:bg-synod-accent/20 transition-all"
        >
          {loading ? 'Provisioning...' : 'Add Slot'}
        </button>
      </div>

      {newAgent && (
        <div className="mb-6 p-4 bg-yellow-400/10 border border-yellow-400/30 rounded-xl relative overflow-hidden">
          <div className="absolute top-0 right-0 p-1 bg-yellow-400 text-black text-[10px] font-bold uppercase px-2">Secret</div>
          <p className="text-xs text-yellow-400 mb-2 font-bold uppercase tracking-widest">New API Key (Save it now!)</p>
          <div className="flex items-center gap-2 bg-black/40 p-3 rounded font-mono text-sm break-all">
            <Key size={14} className="flex-shrink-0" />
            {newAgent.key}
          </div>
        </div>
      )}

      <div className="space-y-3">
        <div className="flex items-center justify-between p-4 bg-black/20 rounded-xl border border-white/5">
          <div className="flex items-center gap-3">
            <Terminal size={18} className="text-gray-500" />
            <div>
              <div className="text-sm font-bold text-white">Default Strategy Agent</div>
              <div className="text-[10px] text-gray-500 font-mono tracking-tighter">ID: ACTIVE</div>
            </div>
          </div>
          <div className="text-[10px] bg-green-500/20 text-green-500 px-2 py-1 rounded-full font-bold uppercase">Online</div>
        </div>
      </div>
    </div>
  );
}
