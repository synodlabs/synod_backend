import { useEffect, useState, useRef } from 'react';
import { BrowserRouter as Router, Routes, Route, Navigate } from 'react-router-dom';
import { Activity, Shield, Wallet, Cpu, LogOut, Bell } from 'lucide-react';
import Login from './components/Login';
import WalletConnect from './components/WalletConnect';
import AgentManager from './components/AgentManager';
import './index.css';

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

function MainDashboard({ token, onLogout }: { token: string, onLogout: () => void }) {
  const [state, setState] = useState<TreasuryState | null>(null);
  const [events, setEvents] = useState<any[]>([]);
  const ws = useRef<WebSocket | null>(null);

  useEffect(() => {
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

    // 2. WebSocket Connection
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/v1/dashboard/ws`;
    ws.current = new WebSocket(wsUrl);

    ws.current.onmessage = (event) => {
      const data = JSON.parse(event.data);
      if (data.type === 'STATE_UPDATE') {
        setState(prev => prev ? { ...prev, ...data.payload } : null);
      }
      setEvents(prev => [data, ...prev].slice(0, 50));
    };

    return () => ws.current?.close();
  }, [token]);

  if (!state) return (
    <div className="min-h-screen bg-synod-bg flex items-center justify-center text-synod-accent font-mono animate-pulse">
      INITIALIZING SYNOD_CORE...
    </div>
  );

  return (
    <div className="min-h-screen bg-synod-bg text-white font-sans selection:bg-synod-accent selection:text-black">
      {/* Sidebar / Nav */}
      <nav className="fixed left-0 top-0 h-full w-20 bg-white/5 border-r border-white/10 flex flex-col items-center py-8 gap-8 backdrop-blur-xl z-50">
        <div className="p-3 bg-synod-accent/20 rounded-xl shadow-[0_0_15px_rgba(0,255,204,0.3)]">
          <Shield className="text-synod-accent w-6 h-6" />
        </div>
        <div className="flex-1 flex flex-col gap-6">
          <div className="p-3 text-synod-accent hover:bg-white/10 rounded-xl transition-all cursor-pointer"><Activity size={24} /></div>
          <div className="p-3 text-gray-500 hover:text-white hover:bg-white/10 rounded-xl transition-all cursor-pointer"><Wallet size={24} /></div>
          <div className="p-3 text-gray-500 hover:text-white hover:bg-white/10 rounded-xl transition-all cursor-pointer"><Cpu size={24} /></div>
        </div>
        <button onClick={onLogout} className="p-3 text-synod-error hover:bg-synod-error/10 rounded-xl transition-all"><LogOut size={24} /></button>
      </nav>

      {/* Main Content */}
      <main className="pl-28 pr-8 py-8 max-w-7xl mx-auto">
        <header className="flex justify-between items-center mb-12">
          <div>
            <h1 className="text-3xl font-black tracking-tight flex items-center gap-3">
              {state.name}
              <span className={`text-[10px] uppercase px-2 py-0.5 rounded-full border ${state.health === 'HEALTHY' ? 'border-synod-accent/50 text-synod-accent bg-synod-accent/10' : 'border-synod-error/50 text-synod-error bg-synod-error/10'}`}>
                {state.health}
              </span>
            </h1>
            <p className="text-gray-500 text-xs mt-1 font-mono">{state.treasury_id}</p>
          </div>
          <div className="flex gap-4">
             <button className="p-2 bg-white/5 border border-white/10 rounded-lg text-gray-400 hover:text-white transition-all"><Bell size={20}/></button>
             <div className="bg-synod-accent/10 border border-synod-accent/30 rounded-lg px-4 py-2 flex items-center gap-3">
                <div className="w-2 h-2 bg-synod-accent rounded-full animate-ping"></div>
                <span className="text-xs font-bold text-synod-accent uppercase tracking-widest">Live Sync</span>
             </div>
          </div>
        </header>

        {/* Stats Grid */}
        <div className="grid grid-cols-1 md:grid-cols-3 gap-6 mb-12">
          <div className="p-6 rounded-2xl bg-white/5 border border-white/10 hover:border-synod-accent/30 transition-all group">
            <h3 className="text-gray-500 text-xs font-bold uppercase tracking-widest mb-1">Treasury AUM</h3>
            <div className="text-4xl font-black group-hover:text-synod-accent transition-colors">${state.current_aum_usd.toLocaleString()}</div>
            <div className="mt-4 flex items-center gap-2 text-[10px] text-gray-500">
               <span className="font-bold">PEAK:</span> ${state.peak_aum_usd.toLocaleString()}
            </div>
          </div>

          <div className="p-6 rounded-2xl bg-white/5 border border-white/10">
            <h3 className="text-gray-500 text-xs font-bold uppercase tracking-widest mb-1">Active Permits</h3>
            <div className="text-4xl font-black text-white/40">--</div>
            <div className="mt-4 flex items-center gap-2 text-[10px] text-synod-accent font-bold">
               POLICY GUARD IS ACTIVE
            </div>
          </div>

          <div className="p-6 rounded-2xl bg-white/5 border border-white/10">
            <h3 className="text-gray-500 text-xs font-bold uppercase tracking-widest mb-1">Network</h3>
            <div className="text-4xl font-black uppercase">{state.network}</div>
            <div className="mt-4 flex items-center gap-2 text-[10px] text-gray-500">
               CONNECTED TO HORIZON_STREAM
            </div>
          </div>
        </div>

        <div className="grid grid-cols-1 lg:grid-cols-3 gap-8">
           <div className="lg:col-span-2 space-y-8">
              <section>
                 <h2 className="text-xl font-bold mb-6 flex items-center gap-3">
                    <Activity className="text-synod-accent" size={20} />
                    Liquidity Allocation
                 </h2>
                 <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    {state.pools.map(pool => (
                      <div key={pool.pool_key} className="p-5 rounded-2xl bg-white/5 border border-white/10 flex justify-between items-center">
                         <div>
                            <div className="text-lg font-bold text-white">{pool.asset_code}</div>
                            <div className="text-[10px] text-gray-500 font-mono italic">{pool.pool_key}</div>
                         </div>
                         <div className="text-right">
                            <div className="text-xl font-black text-synod-accent">{pool.target_pct}%</div>
                            <div className="text-[10px] text-gray-500 font-bold uppercase">Target</div>
                         </div>
                      </div>
                    ))}
                 </div>
              </section>

              <section>
                 <h2 className="text-xl font-bold mb-6">Recent Coordination Events</h2>
                 <div className="rounded-2xl bg-black/40 border border-white/10 overflow-hidden">
                    {events.length === 0 ? (
                      <div className="p-12 text-center text-gray-600 italic">Waiting for incoming signals...</div>
                    ) : (
                      events.map((ev, i) => (
                        <div key={i} className="px-6 py-4 border-b border-white/5 last:border-0 flex items-center justify-between hover:bg-white/5 transition-colors">
                           <div className="flex items-center gap-4">
                              <div className="w-2 h-2 bg-synod-accent rounded-full"></div>
                              <div>
                                 <div className="text-sm font-bold text-white">{ev.type}</div>
                                 <div className="text-[10px] text-gray-500 font-mono">{JSON.stringify(ev.payload).substring(0, 60)}...</div>
                              </div>
                           </div>
                           <div className="text-[10px] font-mono text-gray-600">{new Date().toLocaleTimeString()}</div>
                        </div>
                      ))
                    )}
                 </div>
              </section>
           </div>

           <div>
              <WalletConnect 
                treasuryId={state.treasury_id} 
                token={token} 
                onSuccess={() => console.log('Wallet connected')} 
              />
              <AgentManager 
                treasuryId={state.treasury_id} 
                token={token} 
              />
           </div>
        </div>
      </main>
    </div>
  );
}

export default function App() {
  const [token, setToken] = useState<string | null>(localStorage.getItem('synod_token'));

  const handleLogout = () => {
    localStorage.removeItem('synod_token');
    setToken(null);
  };

  return (
    <Router>
      <Routes>
        <Route path="/login" element={
          token ? <Navigate to="/" /> : <Login onLogin={setToken} />
        } />
        <Route path="/" element={
          token ? <MainDashboard token={token} onLogout={handleLogout} /> : <Navigate to="/login" />
        } />
      </Routes>
    </Router>
  );
}
