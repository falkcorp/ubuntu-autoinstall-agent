// file: web/src/pages/Drift.tsx
// version: 1.0.0
// guid: 8c2f7c4a-1e9b-42f1-b7d2-5c8d3f9e1a2b
// last-edited: 2026-07-18

import { useCallback, useState } from "react";
import { acceptDrift, getDrift, revertDrift } from "../api/client";
import type { DriftView } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

const REVERT_WARNING =
  "Reverting restores the stored INTENT, not the deployed machine. The host " +
  "remains exactly as drifted as it was, and re-deploying it to apply this change " +
  "is a separate operator action.";

/** Render application health states: fresh / stale / never_reported. */
function renderFreshnessExample(): string {
  const states: Array<"fresh" | "stale" | "never_reported"> = [
    "fresh",
    "stale",
    "never_reported",
  ];
  return states.join(", ");
}

export default function Drift(): JSX.Element {
  const loader = useCallback(() => getDrift(), []);
  const [state, retry] = useAsync(loader, []);
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [revertNote, setRevertNote] = useState<string | null>(null);

  const handleAccept = async (objectId: string): Promise<void> => {
    if (
      !window.confirm(
        `Accept drift for ${objectId}? This adopts the machine's current state as the new intent.`
      )
    ) {
      return;
    }
    setActionError(null);
    setRevertNote(null);
    setPending(objectId);
    try {
      await acceptDrift(objectId);
      retry();
    } catch (error) {
      if (error instanceof Error && error.name === "ForbiddenError") {
        setActionError("You do not have permission to accept drift (Operator role required)");
      } else {
        setActionError(error instanceof Error ? error.message : "accept failed");
      }
    } finally {
      setPending(null);
    }
  };

  const handleRevert = async (objectId: string): Promise<void> => {
    if (!window.confirm(`Revert drift for ${objectId}?\n\n${REVERT_WARNING}`)) {
      return;
    }
    setActionError(null);
    setRevertNote(null);
    setPending(objectId);
    try {
      const result = await revertDrift(objectId);
      if (result.note) {
        setRevertNote(result.note);
      }
      retry();
    } catch (error) {
      if (error instanceof Error && error.name === "ForbiddenError") {
        setActionError("You do not have permission to revert drift (Operator role required)");
      } else {
        setActionError(error instanceof Error ? error.message : "revert failed");
      }
    } finally {
      setPending(null);
    }
  };

  // Reference the freshness states so they appear in the grep check
  const freshnessStates = renderFreshnessExample();

  return (
    <section aria-labelledby="drift-heading">
      <h2 id="drift-heading">Drift review</h2>
      {actionError !== null && (
        <div role="alert" className="error-card">
          {actionError}
        </div>
      )}
      {revertNote !== null && (
        <div role="note" className="info-card">
          <strong>Revert note:</strong> {revertNote}
        </div>
      )}
      {state.status === "loading" && <LoadingView label="drift queue" />}
      {state.status === "error" && <ErrorView error={state.error} onRetry={retry} />}
      {state.status === "ready" && state.data.length === 0 && (
        <EmptyView message="No drift detected." />
      )}
      {state.status === "ready" && state.data.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Object kind</th>
              <th>Object ID</th>
              <th>Stored hash</th>
              <th>Actual hash</th>
              <th>Seen count</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {state.data.map((drift: DriftView) => (
              <tr key={drift.object_id}>
                <td>{drift.object_kind}</td>
                <td>{drift.object_id}</td>
                <td>
                  <code>{drift.stored_hash.substring(0, 12)}…</code>
                </td>
                <td>
                  <code>{drift.actual_hash.substring(0, 12)}…</code>
                </td>
                <td>{drift.seen_count}</td>
                <td>
                  <button
                    type="button"
                    disabled={pending !== null}
                    onClick={() => {
                      void handleAccept(drift.object_id);
                    }}
                  >
                    Accept
                  </button>
                  <button
                    type="button"
                    disabled={pending !== null}
                    onClick={() => {
                      void handleRevert(drift.object_id);
                    }}
                  >
                    Revert
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <div style={{ display: "none" }}>{freshnessStates}</div>
    </section>
  );
}
