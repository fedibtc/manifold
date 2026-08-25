// Backup fixtures for the FLIP mock server. Typed against @operator-ui/types;
// keep aligned with the Rust admin surface.

import type { BackupManifest } from '@operator-ui/types';
// The manifest is the real Rust-generated contract fixture (see A4 remediation
// task), so the mock cannot drift from what the daemon's serde impls actually
// produce — in particular the created_at codec and the seven state groups.
// Regenerate via `just gen-contract-fixtures`; never hand-edit the JSON.
import backupManifestFixture from '@operator-ui/types/fixtures/backup_manifest.json';

export const backupManifest: BackupManifest = backupManifestFixture as BackupManifest;
