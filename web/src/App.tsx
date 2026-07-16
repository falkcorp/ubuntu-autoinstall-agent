// file: web/src/App.tsx
// version: 2.0.0
// guid: c0b0d2e6-02e2-4339-b513-0aeac8387103
// last-edited: 2026-07-14

import { useEffect, useState } from "react";
import { NavLink, Route, Routes, useLocation } from "react-router-dom";
import Machines from "./pages/Machines";
import Approvals from "./pages/Approvals";
import Discovery from "./pages/Discovery";
import Audit from "./pages/Audit";
import Login from "./pages/Login";

const SIDEBAR_PINNED_KEY = "uaa-sidebar-pinned";

const NAV_ITEMS = [
  { to: "/machines", label: "Machines", icon: IconServer },
  { to: "/approvals", label: "Pending approvals", icon: IconCheck },
  { to: "/discovery", label: "Discovery inbox", icon: IconRadar },
  { to: "/audit", label: "Audit", icon: IconShield },
];

function IconServer(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.8">
      <rect x="3" y="4" width="18" height="6" rx="1.5" />
      <rect x="3" y="14" width="18" height="6" rx="1.5" />
      <circle cx="7" cy="7" r="0.8" fill="currentColor" stroke="none" />
      <circle cx="7" cy="17" r="0.8" fill="currentColor" stroke="none" />
    </svg>
  );
}

function IconCheck(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.8">
      <circle cx="12" cy="12" r="9" />
      <path d="M8 12.5l2.5 2.5L16 9.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function IconRadar(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.8">
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="5" />
      <path d="M12 12L18 7" strokeLinecap="round" />
    </svg>
  );
}

function IconShield(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.8">
      <path d="M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6l7-3z" strokeLinejoin="round" />
      <path d="M9 12l2 2 4-4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function IconPin({ filled }: { filled: boolean }): JSX.Element {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth="1.8"
    >
      <path d="M12 2l1.5 5.5L19 9l-4.5 3L16 18l-4-3.5L8 18l1.5-6L5 9l5.5-1.5L12 2z" strokeLinejoin="round" />
    </svg>
  );
}

const navLinkClass = ({ isActive }: { isActive: boolean }): string =>
  isActive ? "nav-link nav-link-active" : "nav-link";

function Sidebar({
  pinned,
  onTogglePinned,
}: {
  pinned: boolean;
  onTogglePinned: () => void;
}): JSX.Element {
  const [hovered, setHovered] = useState(false);

  // Expanded when pinned, or (unpinned) while the pointer is over the rail —
  // a hover "pop out" that never touches the persisted pin state, and (see
  // .sidebar-expanded's box-shadow/z-index in index.css) overlays content
  // instead of reflowing it — only a PIN changes the content's own margin.
  const expanded = pinned || hovered;

  return (
    <aside
      className={`sidebar${expanded ? " sidebar-expanded" : " sidebar-collapsed"}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <div className="sidebar-top">
        <div className="sidebar-brand">
          <span className="sidebar-brand-mark">CC</span>
          {expanded && <span className="sidebar-brand-text">Constellation</span>}
        </div>
        <button
          type="button"
          className="sidebar-pin"
          aria-pressed={pinned}
          aria-label={pinned ? "Unpin sidebar" : "Pin sidebar open"}
          title={pinned ? "Unpin sidebar" : "Pin sidebar open"}
          onClick={onTogglePinned}
        >
          <IconPin filled={pinned} />
        </button>
      </div>
      <nav className="sidebar-nav">
        {NAV_ITEMS.map(({ to, label, icon: Icon }) => (
          <NavLink key={to} to={to} className={navLinkClass} title={label}>
            <span className="nav-icon">
              <Icon />
            </span>
            {expanded && <span className="nav-label">{label}</span>}
          </NavLink>
        ))}
      </nav>
    </aside>
  );
}

function TopBanner({ title }: { title: string }): JSX.Element {
  return (
    <header className="app-banner">
      <h1 className="app-banner-title">{title}</h1>
      <div className="app-banner-status">
        <span className="status-dot" aria-hidden="true" />
        <span>Operator plane</span>
      </div>
    </header>
  );
}

const TITLES: Record<string, string> = {
  "/": "Machines",
  "/machines": "Machines",
  "/approvals": "Pending approvals",
  "/discovery": "Discovery inbox",
  "/audit": "Audit log",
};

export default function App(): JSX.Element {
  const location = useLocation();
  const [pinned, setPinned] = useState<boolean>(() => {
    try {
      return localStorage.getItem(SIDEBAR_PINNED_KEY) !== "0";
    } catch {
      return true;
    }
  });

  useEffect(() => {
    try {
      localStorage.setItem(SIDEBAR_PINNED_KEY, pinned ? "1" : "0");
    } catch {
      // best-effort persistence only
    }
  }, [pinned]);

  if (location.pathname === "/login") {
    return (
      <div className="app-shell app-shell-bare">
        <Login />
      </div>
    );
  }

  const title = TITLES[location.pathname] ?? "Constellation Control";

  return (
    <div className="app-shell">
      <Sidebar pinned={pinned} onTogglePinned={() => setPinned((p) => !p)} />
      {/* The content margin tracks PINNED state only — a hover pop-out while
          unpinned overlays the sidebar on top of content instead of shoving
          it sideways (see .sidebar-expanded in index.css). */}
      <div className={`app-content${pinned ? " app-content-pinned" : ""}`}>
        <TopBanner title={title} />
        <main className="app-main">
          <div className="page-card">
            <Routes>
              <Route path="/" element={<Machines />} />
              <Route path="/machines" element={<Machines />} />
              <Route path="/approvals" element={<Approvals />} />
              <Route path="/discovery" element={<Discovery />} />
              <Route path="/audit" element={<Audit />} />
            </Routes>
          </div>
        </main>
      </div>
    </div>
  );
}
