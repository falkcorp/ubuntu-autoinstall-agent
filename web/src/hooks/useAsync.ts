// file: web/src/hooks/useAsync.ts
// version: 1.0.0
// guid: 8a0acbfa-cc7d-4687-bcbe-268eaae2eef5
// last-edited: 2026-07-10

import { useCallback, useEffect, useState } from "react";

export type AsyncState<T> =
  | { status: "loading" }
  | { status: "error"; error: unknown }
  | { status: "ready"; data: T };

/**
 * Fetches `loader()` on mount and whenever `deps` changes, exposing the
 * three states every page must render (loading / error / ready). Call the
 * returned `retry` function from an error card's Retry button to re-run the
 * loader without a full page reload.
 */
export function useAsync<T>(loader: () => Promise<T>, deps: unknown[]): [AsyncState<T>, () => void] {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });
    loader()
      .then((data) => {
        if (!cancelled) {
          setState({ status: "ready", data });
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setState({ status: "error", error });
        }
      });
    return () => {
      cancelled = true;
    };
    // deps intentionally drives re-fetch; loader identity is caller-owned.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return [state, retry];
}
