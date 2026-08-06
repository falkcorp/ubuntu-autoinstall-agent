// file: web/src/pages/Discovery.tsx
// version: 1.2.0
// guid: 4f6e1cfc-fb79-4c90-80d6-1e70e2d29c33
// last-edited: 2026-08-05

import { useCallback, useMemo, useState } from "react";
import { dismissDiscovered, listDiscovered } from "../api/client";
import type { DeviceCategory, DiscoveredMacRow } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

/** A missing category on the wire means the server skipped it as "unknown". */
function categoryOf(row: DiscoveredMacRow): DeviceCategory {
  return row.category ?? "unknown";
}

/** Human label for the category chip. */
function categoryLabel(category: DeviceCategory): string {
  switch (category) {
    case "machine":
      return "Machine";
    case "na":
      return "NA (not a machine)";
    default:
      return "Unknown";
  }
}

export default function Discovery(): JSX.Element {
  const loader = useCallback(() => listDiscovered(), []);
  const [state, retry] = useAsync(loader, []);
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // Non-machine devices (phones/watches/IoT) are noise for install triage, so
  // hide them by default; the operator can reveal them to double-check.
  const [hideNa, setHideNa] = useState(true);

  const allRows = state.status === "ready" ? state.data : [];
  const naCount = useMemo(() => allRows.filter((r) => categoryOf(r) === "na").length, [allRows]);
  const rows = useMemo(
    () => (hideNa ? allRows.filter((r) => categoryOf(r) !== "na") : allRows),
    [allRows, hideNa],
  );

  const handleDismiss = async (mac: string): Promise<void> => {
    if (
      !window.confirm(
        `Mark ${mac} as NOT an install target?\n\n` +
          "This is recorded on the server, so it stays marked across restarts. " +
          "The device is not forgotten — if it keeps appearing on the segment " +
          "its last-seen time still updates, so a marked device behaving " +
          "unexpectedly is still visible.",
      )
    ) {
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
      {state.status === "ready" && (
        <div className="discovery-filter">
          <label>
            <input
              type="checkbox"
              checked={hideNa}
              onChange={(event) => setHideNa(event.target.checked)}
            />{" "}
            Hide auto-detected non-targets (phones, watches, speakers, IoT)
          </label>
          {naCount > 0 && (
            <span className="discovery-filter__count">
              {naCount} device{naCount === 1 ? "" : "s"} auto-classified as
              non-targets from their MAC vendor — {hideNa ? "hidden" : "shown"}
            </span>
          )}
        </div>
      )}
      {state.status === "ready" && rows.length === 0 && (
        <EmptyView
          message={
            allRows.length === 0
              ? "No devices discovered on the segment yet — the inbox is empty."
              : "No machine-class devices to triage (non-machine devices are hidden)."
          }
        />
      )}
      {state.status === "ready" && rows.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Hostname</th>
              <th>IP</th>
              <th>MAC</th>
              <th>Vendor</th>
              <th>Target?</th>
              <th>First seen</th>
              <th>Last seen</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row: DiscoveredMacRow) => {
              const category = categoryOf(row);
              return (
                <tr key={row.mac} className={row.dismissed ? "row--not-target" : undefined}>
                  <td>{row.hostname ?? "—"}</td>
                  <td>{row.ip ?? "—"}</td>
                  <td>
                    <code>{row.mac}</code>
                  </td>
                  <td>{row.vendor ?? "—"}</td>
                  <td>
                    {row.dismissed ? (
                      <span className="category-chip category-chip--na" title="Marked by an operator.">
                        Not a target (marked)
                      </span>
                    ) : (
                      <span
                        className={`category-chip category-chip--${category}`}
                        title={
                          category === "na"
                            ? `Auto-classified from the MAC's OUI${row.vendor ? ` (${row.vendor})` : ""} — phones, watches, speakers and IoT are never install targets.`
                            : category === "machine"
                              ? "Auto-classified as a provisionable machine from the MAC's OUI."
                              : "The MAC's OUI is unknown or randomized, so this could not be classified either way."
                        }
                      >
                        {categoryLabel(category)}
                      </span>
                    )}
                  </td>
                  <td>{row.first_seen}</td>
                  <td>{row.last_seen}</td>
                  <td>
                    <button
                      type="button"
                      disabled={pending === row.mac || row.dismissed}
                      onClick={() => {
                        void handleDismiss(row.mac);
                      }}
                    >
                      {row.dismissed ? "Marked" : "Not a target"}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
