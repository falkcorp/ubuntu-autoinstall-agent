// file: web/src/api/types.ts
// version: 1.3.0
// guid: b7bd1fea-0d99-4db5-a81c-6d6b2a8e9100
// last-edited: 2026-07-23

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
/**
 * Device class derived server-side from the MAC (see crate `oui`).
 * `"na"` = phone/watch/tablet/IoT or a randomized MAC — not an install target.
 * Absent on the wire means `"unknown"` (skipped when unknown to keep the
 * on-disk row clean).
 */
export type DeviceCategory = "machine" | "na" | "unknown";

export interface DiscoveredMacRow {
  mac: string;
  ip?: string | null;
  hostname?: string | null;
  /** IEEE OUI-resolved manufacturer, or null for an unknown/randomized prefix. */
  vendor?: string | null;
  /** Derived device class; treat a missing value as "unknown". */
  category?: DeviceCategory;
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

// ── Profiles (DS-OPS-01) ──────────────────────────────────────────────────

/** Application freshness state — computed client-side from last_app_status_at. */
export type Freshness = "fresh" | "stale" | "never_reported";

/** One row from GET /api/groups (DS-OPS-01). */
export interface HostGroupView {
  id: string;
  name: string;
  hostname_pattern: string;
  is_standalone: boolean;
  defaults: Record<string, unknown>;
  applications: Record<string, unknown>;
  version: number;
  created_at: string | null;
  updated_at: string | null;
}

/** One row from GET /api/groups/:name/profiles (DS-OPS-01). */
export interface HostProfileView {
  id: string;
  group_id: string;
  identity: string;
  hostname_override: string | null;
  overrides: Record<string, unknown>;
  applications: Record<string, unknown>;
  version: number;
  created_at: string | null;
  updated_at: string | null;
}

/** One row from GET /api/groups/:name/allocations (DS-OPS-01). */
export interface AllocationView {
  identity: string;
  index: number;
  hostname: string;
  allocated_at: string | null;
  released_at: string | null;
  rebound_to: string | null;
}

// ── Drift review (DS-OPS-02) ──────────────────────────────────────────────

/** One row from GET /api/drift — a currently-drifted group or profile (DS-OPS-02). */
export interface DriftView {
  object_kind: string;
  object_id: string;
  stored_hash: string;
  actual_hash: string;
  seen_count: number;
}

/** Response of POST /api/drift/:object_id/accept and POST /api/drift/:object_id/revert (DS-OPS-02). */
export interface ReviewResultView {
  object_kind: string;
  object_id: string;
  version: number;
  /** Set ONLY on a revert response; explains that revert restores stored INTENT, not the deployed machine. */
  note: string | null;
}
