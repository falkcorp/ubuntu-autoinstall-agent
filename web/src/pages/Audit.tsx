// file: web/src/pages/Audit.tsx
// version: 1.0.0
// guid: 7753a79a-58a6-45d1-bf57-d07deb08d316
// last-edited: 2026-07-10

import { useCallback } from "react";
import { listAudit, verifyAudit } from "../api/client";
import type { AuditEventRow, AuditVerifyResult } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

interface AuditData {
  events: AuditEventRow[];
  verify: AuditVerifyResult;
}

async function loadAuditData(): Promise<AuditData> {
  const [events, verify] = await Promise.all([listAudit(), verifyAudit()]);
  return { events, verify };
}

// Read-only page: audit events are append-only and this view never mutates
// them, so there are no confirm dialogs here (per the page/API matrix).
export default function Audit(): JSX.Element {
  const loader = useCallback(() => loadAuditData(), []);
  const [state, retry] = useAsync(loader, []);

  return (
    <section aria-labelledby="audit-heading">
      <h2 id="audit-heading">Audit</h2>
      {state.status === "loading" && <LoadingView label="audit log" />}
      {state.status === "error" && <ErrorView error={state.error} onRetry={retry} />}
      {state.status === "ready" && (
        <>
          <VerifyBanner verify={state.data.verify} />
          {state.data.events.length === 0 && <EmptyView message="No audit events recorded yet." />}
          {state.data.events.length > 0 && (
            <table>
              <thead>
                <tr>
                  <th>Seq</th>
                  <th>Actor</th>
                  <th>Action</th>
                  <th>Outcome</th>
                  <th>Timestamp</th>
                  <th>Detail</th>
                </tr>
              </thead>
              <tbody>
                {state.data.events.map((event) => (
                  <tr key={event.seq}>
                    <td>{event.seq}</td>
                    <td>{event.actor}</td>
                    <td>{event.action}</td>
                    <td>{event.outcome}</td>
                    <td>{event.timestamp}</td>
                    <td>{event.detail ?? ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}

function VerifyBanner({ verify }: { verify: AuditVerifyResult }): JSX.Element {
  return (
    <div role="status" className={verify.ok ? "banner banner-verify-ok" : "banner banner-verify-fail"}>
      {verify.ok
        ? `Audit chain verified (${verify.checked} events checked).`
        : `Audit chain verification FAILED: ${verify.message ?? "unknown reason"}`}
    </div>
  );
}
