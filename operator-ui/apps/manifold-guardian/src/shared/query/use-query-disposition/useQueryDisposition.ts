// Promoted to @operator-ui/common-ui so FLIP's screens read the same four
// states. Re-exported here so this dashboard's existing imports keep working
// and stay one line away from the shared definition.
export {
  type QueryDisposition,
  type QueryDispositionModel,
  type QueryRead,
  readQueryDisposition,
  useQueryDisposition
} from '@operator-ui/common-ui';
