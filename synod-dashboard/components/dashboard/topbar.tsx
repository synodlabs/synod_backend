"use client"

import { RefreshCw, Plus, Shield, Bell } from "lucide-react"
import { Button } from "@/components/ui/button"

interface TopbarProps {
  title: string;
  subtitle?: string;
  health: 'HEALTHY' | 'HALTED' | 'DEGRADED' | 'PENDING_WALLET';
  onResync: () => void;
}

export function Topbar({ title, subtitle, health, onResync }: TopbarProps) {
  return (
    <header className="sticky top-0 z-40 bg-synod-bg/80 backdrop-blur-md border-b border-synod-border h-16 flex items-center px-8 gap-4">
      <div className="flex-1">
        <h1 className="text-sm font-bold text-white inline-block">{title}</h1>
        {subtitle && (
          <span className="ml-2 text-[10px] font-mono text-synod-muted-dark uppercase tracking-widest">{subtitle}</span>
        )}
      </div>

      <div className="flex items-center gap-4">
        <div className={`status-pill ${health === 'HEALTHY' ? 'status-pill-healthy' :
            health === 'HALTED' ? 'status-pill-error' :
              'status-pill-warning'
          }`}>
          <div className="dot" />
          {health}
        </div>

        <div className="w-[1px] h-4 bg-synod-border mx-2" />

        <button
          onClick={onResync}
          className="p-2 text-synod-muted hover:text-white transition-colors flex items-center gap-2 text-[11px] font-bold uppercase tracking-wider"
        >
          <RefreshCw size={14} />
          <span className="hidden sm:inline">Resync</span>
        </button>

        <div className="w-[1px] h-4 bg-synod-border mx-2" />

        <button className="p-2 text-synod-muted hover:text-white transition-colors">
          <Bell size={18} />
        </button>
      </div>
    </header>
  )
}
