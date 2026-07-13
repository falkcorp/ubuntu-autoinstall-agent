// file: web/src/api/types.ts
// version: 1.1.0
// guid: b7bd1fea-0d99-4db5-a81c-6d6b2a8e9100
// last-edited: 2026-07-13

// Typed DTOs mirroring CT-07's operator API responses. Names are kept
// identical to CT-07's api_types.rs (MachineRow, EnrollmentRow,
// DiscoveredMacRow, AuditEventRow) so the two sides stay name-aligned even
// though CT-07 has not landed yet (spec Decision 19).

/** One row from GET /api/machines and GET /api/machines/{mac}. */
export interface MachineRow {
  mac: string;
  hostname: string;
  status: string;
  boot_target: string;
  tpm_ek: string | null;
  /** True when every provisioning layer for this machine agrees; false = drift. */
  consistent: boolean;
  last_seen: string;
}

/** One row from GET /api/enrollments (pending enrollment CSRs). */
export interface EnrollmentRow {
  spki_fingerprint: string;
  claimed_mac: string;
  claimed_hostname: string;
  state: string;
  first_seen: string;
}

/** One row from GET /api/discovered (unknown PXE MACs / discovery inbox). */
export interface DiscoveredMacRow {
  mac: string;
  first_seen: string;
  last_seen: string;
  dismissed: boolean;
}

/** One row from GET /api/audit (chained audit log). */
export interface AuditEventRow {
  seq: number;
  actor: string;
  action: string;
  outcome: string;
  timestamp: string;
  detail: string | null;
}

/** Result of GET /api/audit/verify — audit chain integrity check. */
export interface AuditVerifyResult {
  ok: boolean;
  checked: number;
  message: string | null;
}

/** Shape of an error body returned by the operator API on non-2xx responses. */
export interface ApiErrorBody {
  message: string;
}

/** Result of GET /api/auth/status. */
export interface AuthStatus {
  authenticated: boolean;
  bootstrap_token_enabled: boolean;
}

/** Thrown by apiFetch for any non-2xx, non-401, non-403 response. */
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/** Thrown by apiFetch on 403 — caller renders an inline banner, no redirect. */
export class ForbiddenError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ForbiddenError";
  }
}
