// file: web/src/api/client.ts
// version: 1.2.0
// guid: a7e23f11-4508-4940-aa29-8b66b7a3d28d
// last-edited: 2026-07-18

import {
  ApiError,
  ForbiddenError,
  type AllocationView,
  type ApiErrorBody,
  type AuditEventRow,
  type AuditVerifyResult,
  type AuthStatus,
  type DiscoveredMacRow,
  type DriftView,
  type EnrollmentRow,
  type HostGroupView,
  type HostProfileView,
  type MachineRow,
  type ReviewResultView,
} from "./types";

export { ApiError, ForbiddenError };

/**
 * Single fetch wrapper used by every typed helper below. Implements the
 * pinned edge-case law (spec C3 + Decision 19), spelled out once here:
 *
 *   - 401 Unauthorized -> redirect the whole page to /login (this SPA's own
 *     login page, NOT the backend's /auth/login OAuth-initiator route
 *     directly — that 302s straight to GitHub, which is broken/unhelpful
 *     until a real OAuth app is configured; /login lets the user choose SSO
 *     or the bootstrap-token stopgap). Session auth (GitHub OAuth or the
 *     bootstrap token, CT-03) lives entirely server-side; the SPA never
 *     stores a token and never retries — it just bounces to the login page.
 *   - 403 Forbidden -> throw ForbiddenError so the caller can render an
 *     inline "insufficient role" banner. This must NOT redirect, or an
 *     under-privileged but authenticated user would loop forever between
 *     the app and /auth/login.
 *   - Any other non-2xx -> throw ApiError with the server-provided message
 *     (falling back to statusText) so pages can show a retry-able error
 *     card.
 *   - 204 / empty body -> resolved as `undefined as T` for mutation
 *     endpoints that return no content.
 */
export async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      Accept: "application/json",
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  });

  if (response.status === 401) {
    window.location.href = "/login";
    // The redirect above unwinds the page; this promise never needs to
    // resolve, but TypeScript still requires a return path.
    return new Promise<T>(() => {});
  }

  if (response.status === 403) {
    const body = await safeParseError(response);
    throw new ForbiddenError(body?.message ?? "insufficient role");
  }

  if (!response.ok) {
    const body = await safeParseError(response);
    throw new ApiError(response.status, body?.message ?? response.statusText);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  if (text.length === 0) {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

async function safeParseError(response: Response): Promise<ApiErrorBody | null> {
  try {
    return (await response.json()) as ApiErrorBody;
  } catch {
    return null;
  }
}

// ---- Machines --------------------------------------------------------

export function listMachines(): Promise<MachineRow[]> {
  return apiFetch<MachineRow[]>("/api/machines");
}

export function getMachine(mac: string): Promise<MachineRow> {
  return apiFetch<MachineRow>(`/api/machines/${encodeURIComponent(mac)}`);
}

export function approveMachine(mac: string): Promise<void> {
  return apiFetch<void>(`/api/machines/${encodeURIComponent(mac)}/approve`, {
    method: "POST",
  });
}

/**
 * `confirm` must be `true` — it is the explicit, server-checked
 * acknowledgement that the caller has seen the reinstall cooldown warning
 * (rendered client-side in the Machines page's confirm dialog) and wants to
 * proceed anyway. This is separate from, and in addition to, the
 * client-side `window.confirm` dialog: the dialog stops accidental clicks,
 * this flag lets the server reject a stale/forged request that skipped it.
 */
export function reinstallMachine(mac: string, confirm: true): Promise<void> {
  return apiFetch<void>(`/api/machines/${encodeURIComponent(mac)}/reinstall`, {
    method: "POST",
    body: JSON.stringify({ confirm }),
  });
}

// ---- Enrollments (CSRs) ------------------------------------------------

export function listEnrollments(): Promise<EnrollmentRow[]> {
  return apiFetch<EnrollmentRow[]>("/api/enrollments");
}

export function approveEnrollment(fp: string): Promise<void> {
  return apiFetch<void>(`/api/enrollments/${encodeURIComponent(fp)}/approve`, {
    method: "POST",
  });
}

export function rejectEnrollment(fp: string): Promise<void> {
  return apiFetch<void>(`/api/enrollments/${encodeURIComponent(fp)}/reject`, {
    method: "POST",
  });
}

// ---- Discovery inbox ----------------------------------------------------

export function listDiscovered(): Promise<DiscoveredMacRow[]> {
  return apiFetch<DiscoveredMacRow[]>("/api/discovered");
}

export function dismissDiscovered(mac: string): Promise<void> {
  return apiFetch<void>(`/api/discovered/${encodeURIComponent(mac)}/dismiss`, {
    method: "POST",
  });
}

// ---- Audit ---------------------------------------------------------------

export function listAudit(): Promise<AuditEventRow[]> {
  return apiFetch<AuditEventRow[]>("/api/audit");
}

export function verifyAudit(): Promise<AuditVerifyResult> {
  return apiFetch<AuditVerifyResult>("/api/audit/verify");
}

// ---- Auth (CT-03: GitHub SSO + a temporary bootstrap-token stopgap) -----

/** Always public — never triggers apiFetch's 401 redirect. */
export function getAuthStatus(): Promise<AuthStatus> {
  return apiFetch<AuthStatus>("/api/auth/status");
}

/** Thrown by {@link bootstrapLogin} on a wrong/expired/reused/disabled token. */
export class BootstrapLoginError extends Error {}

/**
 * Exchanges a bootstrap token for a real session cookie. Deliberately does
 * NOT go through {@link apiFetch} — its global 401 handling is for "your
 * session expired while using the app" (redirect to /login), which is the
 * wrong behavior for "you typed the wrong token" (show an inline error on
 * the login page instead).
 */
export async function bootstrapLogin(token: string): Promise<void> {
  const response = await fetch("/api/auth/bootstrap", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (!response.ok) {
    throw new BootstrapLoginError("Invalid, expired, or already-used token.");
  }
}

/** Admin-only: permanently retires the bootstrap-token login path. */
export function disableBootstrapToken(): Promise<void> {
  return apiFetch<void>("/api/auth/bootstrap/disable", { method: "POST" });
}

// ---- Profiles (DS-OPS-01) --------------------------------------------------

export function getGroups(): Promise<HostGroupView[]> {
  return apiFetch<HostGroupView[]>("/api/groups");
}

export function getGroupProfiles(name: string): Promise<HostProfileView[]> {
  return apiFetch<HostProfileView[]>(`/api/groups/${encodeURIComponent(name)}/profiles`);
}

export function getAllocations(name: string): Promise<AllocationView[]> {
  return apiFetch<AllocationView[]>(`/api/groups/${encodeURIComponent(name)}/allocations`);
}

// ---- Drift review (DS-OPS-02) -----------------------------------------------

export function getDrift(): Promise<DriftView[]> {
  return apiFetch<DriftView[]>("/api/drift");
}

export function acceptDrift(objectId: string): Promise<ReviewResultView> {
  return apiFetch<ReviewResultView>(`/api/drift/${encodeURIComponent(objectId)}/accept`, {
    method: "POST",
  });
}

export function revertDrift(objectId: string): Promise<ReviewResultView> {
  return apiFetch<ReviewResultView>(`/api/drift/${encodeURIComponent(objectId)}/revert`, {
    method: "POST",
  });
}
