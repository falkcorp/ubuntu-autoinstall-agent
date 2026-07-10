// file: web/src/pages/Approvals.tsx
// version: 1.0.0
// guid: dc26abfa-1fea-4f49-85d1-f52de2910f95
// last-edited: 2026-07-10

import { useCallback, useState } from "react";
import {
  approveEnrollment,
  approveMachine,
  listDiscovered,
  listEnrollments,
  listMachines,
  rejectEnrollment,
} from "../api/client";
import type { DiscoveredMacRow, EnrollmentRow, MachineRow } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

interface ApprovalsData {
  machines: MachineRow[];
  enrollments: EnrollmentRow[];
  discovered: DiscoveredMacRow[];
}

async function loadApprovalsData(): Promise<ApprovalsData> {
  const [machines, enrollments, discovered] = await Promise.all([
    listMachines(),
    listEnrollments(),
    listDiscovered(),
  ]);
  return { machines, enrollments, discovered };
}

/** Correlate a claimed MAC against the discovery inbox, per spec C6. */
function findDiscoveryMatch(discovered: DiscoveredMacRow[], mac: string): DiscoveredMacRow | undefined {
  return discovered.find((row) => row.mac.toLowerCase() === mac.toLowerCase());
}

export default function Approvals(): JSX.Element {
  const loader = useCallback(() => loadApprovalsData(), []);
  const [state, retry] = useAsync(loader, []);
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const handleApproveMachine = async (mac: string): Promise<void> => {
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

  const handleApproveEnrollment = async (fp: string): Promise<void> => {
    if (!window.confirm(`Approve enrollment CSR ${fp}?`)) {
      return;
    }
    setActionError(null);
    setPending(fp);
    try {
      await approveEnrollment(fp);
      retry();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "approve failed");
    } finally {
      setPending(null);
    }
  };

  const handleRejectEnrollment = async (fp: string): Promise<void> => {
    if (!window.confirm(`Reject enrollment CSR ${fp}?`)) {
      return;
    }
    setActionError(null);
    setPending(fp);
    try {
      await rejectEnrollment(fp);
      retry();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "reject failed");
    } finally {
      setPending(null);
    }
  };

  return (
    <section aria-labelledby="approvals-heading">
      <h2 id="approvals-heading">Pending approvals</h2>
      {actionError !== null && (
        <div role="alert" className="error-card">
          {actionError}
        </div>
      )}
      {state.status === "loading" && <LoadingView label="pending approvals" />}
      {state.status === "error" && <ErrorView error={state.error} onRetry={retry} />}
      {state.status === "ready" && (
        <>
          <ApprovalsMachinesSection
            machines={state.data.machines.filter((m) => m.status === "pending")}
            pending={pending}
            onApprove={handleApproveMachine}
          />
          <ApprovalsEnrollmentsSection
            enrollments={state.data.enrollments.filter((e) => e.state === "pending")}
            discovered={state.data.discovered}
            pending={pending}
            onApprove={handleApproveEnrollment}
            onReject={handleRejectEnrollment}
          />
        </>
      )}
    </section>
  );
}

function ApprovalsMachinesSection({
  machines,
  pending,
  onApprove,
}: {
  machines: MachineRow[];
  pending: string | null;
  onApprove: (mac: string) => void;
}): JSX.Element {
  return (
    <div className="approvals-section">
      <h3>Pending machine approvals</h3>
      {machines.length === 0 && <EmptyView message="No pending machine approvals." />}
      {machines.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Hostname</th>
              <th>MAC</th>
              <th>Boot target</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {machines.map((machine) => (
              <tr key={machine.mac}>
                <td>{machine.hostname}</td>
                <td>{machine.mac}</td>
                <td>{machine.boot_target}</td>
                <td>
                  <button type="button" disabled={pending === machine.mac} onClick={() => onApprove(machine.mac)}>
                    Approve
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function ApprovalsEnrollmentsSection({
  enrollments,
  discovered,
  pending,
  onApprove,
  onReject,
}: {
  enrollments: EnrollmentRow[];
  discovered: DiscoveredMacRow[];
  pending: string | null;
  onApprove: (fp: string) => void;
  onReject: (fp: string) => void;
}): JSX.Element {
  return (
    <div className="approvals-section">
      <h3>Pending enrollment CSRs</h3>
      {enrollments.length === 0 && <EmptyView message="No pending enrollment requests." />}
      {enrollments.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>SPKI fingerprint</th>
              <th>Claimed MAC</th>
              <th>Claimed hostname</th>
              <th>Discovery inbox match</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {enrollments.map((enrollment) => {
              const match = findDiscoveryMatch(discovered, enrollment.claimed_mac);
              return (
                <tr key={enrollment.spki_fingerprint}>
                  <td>{enrollment.spki_fingerprint}</td>
                  <td>{enrollment.claimed_mac}</td>
                  <td>{enrollment.claimed_hostname}</td>
                  <td>
                    {match ? (
                      <span className="badge badge-match">seen {match.first_seen}</span>
                    ) : (
                      <span className="badge badge-nomatch">no discovery match</span>
                    )}
                  </td>
                  <td>
                    <button
                      type="button"
                      disabled={pending === enrollment.spki_fingerprint}
                      onClick={() => onApprove(enrollment.spki_fingerprint)}
                    >
                      Approve
                    </button>
                    <button
                      type="button"
                      disabled={pending === enrollment.spki_fingerprint}
                      onClick={() => onReject(enrollment.spki_fingerprint)}
                    >
                      Reject
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}
