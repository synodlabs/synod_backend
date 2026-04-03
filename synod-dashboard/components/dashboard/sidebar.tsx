"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import {
  LayoutDashboard,
  Wallet,
  Cpu,
  ShieldCheck,
  FileText,
  History,
  Settings,
  Shield,
  Target
} from "lucide-react"

interface SidebarItem {
  id: string;
  label: string;
  icon: React.ReactNode;
  badge?: number;
  badgeColor?: string;
}

interface SidebarProps {
  activeTab: string;
  onTabChange: (tab: any) => void;
  user: {
    name: string;
    email?: string;
    avatar?: string;
  };
  badges: {
    wallets: number;
    agents: number;
    permits: number;
  };
}

export function Sidebar({ activeTab, onTabChange, user, badges }: SidebarProps) {
  const navSections = [
    {
      label: "Core",
      items: [
        { id: "overview", label: "Overview", icon: <LayoutDashboard size={18} /> },
        { id: "wallets", label: "Wallets", icon: <Wallet size={18} />, badge: badges.wallets },
        { id: "agents", label: "Agents", icon: <Cpu size={18} />, badge: badges.agents },
      ]
    },
    {
      label: "Governance",
      items: [
        { id: "policy", label: "Policy & Rules", icon: <ShieldCheck size={18} /> },
        { id: "permits", label: "Permits", icon: <FileText size={18} />, badge: badges.permits, badgeColor: "amber" },
      ]
    },
    {
      label: "System",
      items: [
        { id: "activity", label: "Activity Log", icon: <History size={18} /> },
        { id: "settings", label: "Settings", icon: <Settings size={18} /> },
      ]
    }
  ]

  return (
    <aside className="w-64 min-w-[256px] bg-synod-card border-r border-synod-border flex flex-col h-screen sticky top-0">
      <div className="p-6 border-b border-synod-border flex flex-col items-start gap-1">
        <img 
          src="/synod_logo.png" 
          alt="Synod" 
          className="h-5 w-auto object-contain -ml-0.5" 
        />
        <div className="font-mono text-[8px] text-synod-muted uppercase tracking-[0.2em] mt-1">
          Capital Governance
        </div>
      </div>

      <nav className="flex-1 px-4 py-6 overflow-y-auto space-y-8">
        {navSections.map((section) => (
          <div key={section.label} className="space-y-1">
            <h3 className="px-3 font-mono text-[9px] text-synod-muted-dark uppercase tracking-widest mb-3">
              {section.label}
            </h3>
            <div className="space-y-0.5">
              {section.items.map((item: SidebarItem) => (
                <button
                  key={item.id}
                  onClick={() => onTabChange(item.id)}
                  className={`w-full flex items-center gap-3 px-3 py-2.5 rounded-sm transition-all duration-200 group relative ${activeTab === item.id
                    ? "bg-white/5 text-white shadow-sm"
                    : "text-synod-muted hover:text-white hover:bg-white/[0.03]"
                    }`}
                >
                  {activeTab === item.id && (
                    <div className="absolute left-0 top-1/2 -translate-y-1/2 w-[2px] h-4 bg-white rounded-r-full" />
                  )}
                  <span className={`${activeTab === item.id ? "text-white" : "text-synod-muted-dark group-hover:text-synod-muted"}`}>
                    {item.icon}
                  </span>
                  <span className="text-[13px] font-medium tracking-tight">
                    {item.label}
                  </span>
                  {item.badge && (
                    <span className={`ml-auto text-[9px] font-mono font-bold px-1.5 py-0.5 rounded-full ${item.badgeColor === "amber"
                      ? "bg-[#F5A623] text-black"
                      : "bg-white/10 text-white"
                      }`}>
                      {item.badge}
                    </span>
                  )}
                </button>
              ))}
            </div>
          </div>
        ))}
      </nav>

      <div className="p-4 border-t border-synod-border">
        <div className="flex items-center gap-3 p-2 rounded-lg hover:bg-white/5 transition-colors cursor-pointer group">
          <div className="w-8 h-8 rounded-full bg-zinc-800 border border-zinc-700 flex items-center justify-center text-[10px] font-bold text-white uppercase overflow-hidden">
            {user.avatar || user.name.substring(0, 2)}
          </div>
          <div className="flex-1 min-w-0">
            <div className="text-xs font-bold text-white truncate">{user.name}</div>
            <div className="text-[9px] text-synod-muted-dark font-mono uppercase tracking-wider truncate">
              Mainnet · Treasury-1
            </div>
          </div>
          <Settings size={14} className="text-synod-muted-dark group-hover:text-white transition-colors" />
        </div>
      </div>
    </aside>
  )
}
