// file: web/src/pages/Discovery.tsx
// version: 1.0.0
// guid: 4f6e1cfc-fb79-4c90-80d6-1e70e2d29c33
// last-edited: 2026-07-10

import { useCallback, useState } from "react";
import { dismissDiscovered, listDiscovered } from "../api/client";
import type { DiscoveredMacRow } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

export default function Discovery(): JSX.Element {
  const loader = useCallback(() => listDiscovered(), []);
  const [state, retry] = useAsync(loader, []);
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const handleDismiss = async (mac: string): Promise<void> => {
    if (!window.confirm(`Dismiss unknown PXE MAC ${mac} from the discovery inbox?`)) {
      return;
    }
    setActionError(null);
    setPending(mac);
    try {
      await dismissDiscovered(mac);
      retry();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "dismiss failed");
    } finally {
      setPending(null);
    }
  };

  return (
    <section aria-labelledby="discovery-heading">
      <h2 id="discovery-heading">Discovery inbox</h2>
      {actionError !== null && (
        <div role="alert" className="error-card">
          {actionError}
        </div>
      )}
      {state.status === "loading" && <LoadingView label="discovery inbox" />}
      {state.status === "error" && <ErrorView error={state.error} onRetry={retry} />}
      {state.status === "ready" && state.data.length === 0 && (
        <EmptyView message="No unrecognized PXE MACs — the discovery inbox is empty." />
      )}
      {state.status === "ready" && state.data.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>MAC</th>
              <th>First seen</th>
              <th>Last seen</th>
              <th>Dismissed</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {state.data.map((row: DiscoveredMacRow) => (
              <tr key={row.mac}>
                <td>{row.mac}</td>
                <td>{row.first_seen}</td>
                <td>{row.last_seen}</td>
                <td>{row.dismissed ? "yes" : "no"}</td>
                <td>
                  <button
                    type="button"
                    disabled={pending === row.mac || row.dismissed}
                    onClick={() => {
                      void handleDismiss(row.mac);
                    }}
                  >
                    Dismiss
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
