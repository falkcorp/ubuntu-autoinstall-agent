// file: web/src/pages/Login.tsx
// version: 1.0.0
// guid: 3f7b7c1a-9c2e-4b1d-8a3e-6d9f2c5e8a41
// last-edited: 2026-07-13

import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { bootstrapLogin, getAuthStatus } from "../api/client";
import type { AuthStatus } from "../api/types";

/**
 * Standalone login page (not gated by apiFetch's own 401 handling — that
 * would loop). Offers the real long-term path (GitHub SSO, a full-page
 * navigation to the backend's OAuth-initiating `/auth/login`) alongside the
 * temporary bootstrap-token stopgap (CT-03's `crate::auth` module doc has
 * the full story on why that exists and how it's disabled).
 */
export default function Login(): JSX.Element {
  const navigate = useNavigate();
  const [status, setStatus] = useState<AuthStatus | null>(null);
  const [token, setToken] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getAuthStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch(() => {
        // Status itself failing means the operator plane is unreachable —
        // leave `status` null; the page still renders the SSO button, which
        // is a plain link and doesn't depend on this fetch succeeding.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (status?.authenticated) {
      navigate("/", { replace: true });
    }
  }, [status, navigate]);

  const handleBootstrapSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      setSubmitting(true);
      setError(null);
      try {
        await bootstrapLogin(token);
        navigate("/", { replace: true });
      } catch {
        setError("Invalid, expired, or already-used token.");
      } finally {
        setSubmitting(false);
      }
    },
    [token, navigate],
  );

  return (
    <section aria-labelledby="login-heading" className="login-page">
      <h1 id="login-heading">Constellation Control</h1>

      <div className="login-option">
        <h2>Sign in with GitHub</h2>
        <p>The standard login path — GitHub org/team membership determines your role.</p>
        {/* Full-page navigation, not a fetch: this hits the backend's OAuth
            redirect (GET /auth/login), which 302s to GitHub. */}
        <a className="button button-primary" href="/auth/login">
          Sign in with GitHub SSO
        </a>
      </div>

      {status?.bootstrap_token_enabled && (
        <div className="login-option">
          <h2>Bootstrap admin token</h2>
          <p>
            Temporary stopgap for while no GitHub OAuth app is configured yet. The token is
            single-use, short-lived, and printed to the <code>uaa-control</code> service log (also
            written to <code>/var/lib/uaa/operator-bootstrap-token</code>).
          </p>
          <form onSubmit={handleBootstrapSubmit}>
            <label htmlFor="bootstrap-token">Token</label>
            <input
              id="bootstrap-token"
              name="token"
              type="text"
              autoComplete="off"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              disabled={submitting}
              required
            />
            <button type="submit" disabled={submitting || token.length === 0}>
              {submitting ? "Signing in…" : "Sign in"}
            </button>
          </form>
          {error && (
            <div role="alert" className="banner banner-error">
              {error}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
