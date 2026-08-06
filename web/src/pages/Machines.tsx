// file: web/src/pages/Machines.tsx
// version: 1.2.0
// guid: a19e686a-0212-4f7a-a751-25d7d67e6acf
// last-edited: 2026-08-05

import { useCallback, useState } from "react";
import { approveMachine, listMachines, reinstallMachine } from "../api/client";
import type { MachineRow } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

/** Render a unix-epoch string (what the API stores) as a local date-time. */
function formatSeen(epoch: string): string {
  const secs = Number(epoch);
  if (!epoch || Number.isNaN(secs) || secs === 0) {
    return "—";
  }
  return new Date(secs * 1000).toLocaleString();
}

/**
 * Agent-liveness wording. "stale" and "never" are deliberately NOT rendered as
 * failures: neither means the machine is unhealthy, only that nothing recent
 * has been reported. Claiming more than that is what the old always-true
 * "consistent" badge did.
 */
const AGENT_LABEL: Record<MachineRow["agent"], string> = {
  reporting: "reporting",
  stale: "stale",
  never: "no agent",
};

const AGENT_TITLE: Record<MachineRow["agent"], string> = {
  reporting: "Checked in within the last 15 minutes.",
  stale: "Has reported before, but not recently — current state unknown.",
  never: "Has never reported. Either the agent is not installed, or it has never run.",
};

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
              <th>IP</th>
              <th>MAC</th>
              <th>Status</th>
              <th>Boot target</th>
              <th>Agent</th>
              <th>Last seen</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {state.data.map((machine: MachineRow) => (
              <tr key={machine.mac}>
                <td>{machine.hostname}</td>
                <td>{machine.ip ?? "—"}</td>
                <td>{machine.mac}</td>
                <td>
                  <span className={`badge badge-${machine.status}`}>{machine.status}</span>
                </td>
                <td>{machine.boot_target}</td>
                <td>
                  <span className={`badge badge-agent-${machine.agent}`} title={AGENT_TITLE[machine.agent]}>
                    {AGENT_LABEL[machine.agent]}
                  </span>
                </td>
                <td>{formatSeen(machine.last_seen)}</td>
                <td>
                  {machine.status !== "approved" && (
                    <button
                      type="button"
                      disabled={pending === machine.mac}
                      onClick={() => {
                        void handleApprove(machine.mac);
                      }}
                    >
                      Approve
                    </button>
                  )}
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
