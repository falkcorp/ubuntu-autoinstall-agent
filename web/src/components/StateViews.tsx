// file: web/src/components/StateViews.tsx
// version: 1.0.0
// guid: 51f53138-d953-4e1b-9d3c-f338469a6184
// last-edited: 2026-07-10

import { ForbiddenError } from "../api/client";

/** Shown while a page's initial (or retried) fetch is in flight. */
export function LoadingView({ label }: { label: string }): JSX.Element {
  return (
    <p role="status" className="state-loading">
      Loading {label}…
    </p>
  );
}

/**
 * Renders the two non-2xx failure shapes a page can hit:
 *   - ForbiddenError (403): inline "insufficient role" banner, NO retry
 *     button that would just 403 again in a loop, and definitely no
 *     redirect (401 already redirected inside apiFetch before this ever
 *     renders).
 *   - Anything else (network error, ApiError, ...): a retry-able error
 *     card, since these are plausibly transient.
 */
export function ErrorView({ error, onRetry }: { error: unknown; onRetry: () => void }): JSX.Element {
  if (error instanceof ForbiddenError) {
    return (
      <div role="alert" className="banner banner-forbidden">
        Insufficient role: {error.message}
      </div>
    );
  }
  const message = error instanceof Error ? error.message : "Unknown error";
  return (
    <div role="alert" className="error-card">
      <p>Failed to load: {message}</p>
      <button type="button" onClick={onRetry}>
        Retry
      </button>
    </div>
  );
}

/** Explicit empty state — a list page must never just render a blank page. */
export function EmptyView({ message }: { message: string }): JSX.Element {
  return <p className="empty-state">{message}</p>;
}
