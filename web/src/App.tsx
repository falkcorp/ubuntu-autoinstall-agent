// file: web/src/App.tsx
// version: 1.0.0
// guid: c0b0d2e6-02e2-4339-b513-0aeac8387103
// last-edited: 2026-07-10

import { NavLink, Route, Routes } from "react-router-dom";
import Machines from "./pages/Machines";
import Approvals from "./pages/Approvals";
import Discovery from "./pages/Discovery";
import Audit from "./pages/Audit";

const navLinkClass = ({ isActive }: { isActive: boolean }): string =>
  isActive ? "nav-link nav-link-active" : "nav-link";

export default function App(): JSX.Element {
  return (
    <div className="app-shell">
      <header className="app-header">
        <h1>Constellation Control</h1>
        <nav>
          <NavLink to="/machines" className={navLinkClass}>
            Machines
          </NavLink>
          <NavLink to="/approvals" className={navLinkClass}>
            Pending approvals
          </NavLink>
          <NavLink to="/discovery" className={navLinkClass}>
            Discovery inbox
          </NavLink>
          <NavLink to="/audit" className={navLinkClass}>
            Audit
          </NavLink>
        </nav>
      </header>
      <main className="app-main">
        <Routes>
          <Route path="/" element={<Machines />} />
          <Route path="/machines" element={<Machines />} />
          <Route path="/approvals" element={<Approvals />} />
          <Route path="/discovery" element={<Discovery />} />
          <Route path="/audit" element={<Audit />} />
        </Routes>
      </main>
    </div>
  );
}
