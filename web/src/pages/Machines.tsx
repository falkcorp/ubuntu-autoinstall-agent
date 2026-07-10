// file: web/src/pages/Machines.tsx
// version: 1.0.1
// guid: a19e686a-0212-4f7a-a751-25d7d67e6acf
// last-edited: 2026-07-10

import { useCallback, useState } from "react";
import { approveMachine, listMachines, reinstallMachine } from "../api/client";
import type { MachineRow } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

const REINSTALL_COOLDOWN_WARNING =
  "Reinstalling wipes and re-provisions this machine from scratch. A machine " +
  "that was just reinstalled is subject to a cooldown before another " +
  "reinstall can be requested — repeated reinstalls in a short window will " +
  "be rejected by the server.";

export default function Machines(): JSX.Element {
  const loader = useCallback(() => listMachines(), []);
  const [state, retry] = useAsync(loader, []);
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const handleApprove = async (mac: string): Promise<void> => {
    if (!window.confirm(`Approve machine ${mac}?`)) {
      return;
    }
    setActionError(null);
    setPending(mac);
    try {
      await approveMachine(mac);
      retry();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "approve failed");
    } finally {
      setPending(null);
    }
  };

  const handleReinstall = async (mac: string): Promise<void> => {
    if (!window.confirm(`Reinstall machine ${mac}?\n\n${REINSTALL_COOLDOWN_WARNING}`)) {
      return;
    }
    setActionError(null);
    setPending(mac);
    try {
      await reinstallMachine(mac, true);
      retry();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "reinstall failed");
    } finally {
      setPending(null);
    }
  };

  return (
    <section aria-labelledby="machines-heading">
      <h2 id="machines-heading">Machines</h2>
      {actionError !== null && (
        <div role="alert" className="error-card">
          {actionError}
        </div>
      )}
      {state.status === "loading" && <LoadingView label="machines" />}
      {state.status === "error" && <ErrorView error={state.error} onRetry={retry} />}
      {state.status === "ready" && state.data.length === 0 && <EmptyView message="No machines yet." />}
      {state.status === "ready" && state.data.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Hostname</th>
              <th>MAC</th>
              <th>Status</th>
              <th>Boot target</th>
              <th>Consistent</th>
              <th>Last seen</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {state.data.map((machine: MachineRow) => (
              <tr key={machine.mac}>
                <td>{machine.hostname}</td>
                <td>{machine.mac}</td>
                <td>
                  <span className={`badge badge-${machine.status}`}>{machine.status}</span>
                </td>
                <td>{machine.boot_target}</td>
                <td>
                  {machine.consistent ? (
                    <span className="badge badge-consistent">consistent</span>
                  ) : (
                    <span className="badge badge-drift">drift</span>
                  )}
                </td>
                <td>{machine.last_seen}</td>
                <td>
                  <button
                    type="button"
                    disabled={pending === machine.mac}
                    onClick={() => {
                      void handleApprove(machine.mac);
                    }}
                  >
                    Approve
                  </button>
                  <button
                    type="button"
                    disabled={pending === machine.mac}
                    onClick={() => {
                      void handleReinstall(machine.mac);
                    }}
                  >
                    Reinstall
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
