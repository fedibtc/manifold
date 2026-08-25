// Lifted to shared (allocations also needs it and feature-boundaries forbid
// importing across features); re-exported here so the existing funds
// importers (TopupPanel, FundsActions) are unaffected.
export { describeActionError } from '@/shared/utils/describeActionError';
