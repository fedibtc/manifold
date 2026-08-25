// Generic paging types shared by admin list APIs (requests, allocations, wallet
// operations, …). Mirrors crates/service-liquidity-manager/src/provisional_types.rs.
// Hand-maintained: keep aligned with the Rust wire shapes.

import type { Timestamp } from './admin';

// PageCursor is serde(transparent) → a bare string on the wire.
export type PageCursor = string;

export interface PageRequest {
  cursor?: PageCursor | null;
  limit: number; // u32
}

export interface TimeRange {
  from?: Timestamp | null;
  to?: Timestamp | null;
}

export interface ListResponse<T> {
  items: T[];
  next_page?: PageCursor | null;
}
