# Deploy the cloud FMan telemetry collector

This is the secure-deployment entry point for the cloud FMan telemetry
collector. The collector runs as one active process over one encrypted
persistent volume.

The software production-readiness assessment assumes the controls below; it
does **not** verify a particular cluster, registry, secret manager, storage
system, Prometheus backend, backup system, or operator procedure. Satisfying
this guide's controls is necessary, but not by itself sufficient, for a
deployment to fall inside that assessment. A deployment that does not satisfy
them is outside the collector's supported production envelope, even if the
process starts.

## Secure-deployment contract and checklist

Before an operator calls a deployment production, it should record evidence for
every item. The startup gates only the items explicitly called out below; a
checked box is an operator assertion, not evidence supplied by this repository.

- [ ] **Artifact correspondence:** select the collector by immutable registry
  digest, establish that its manifest, configuration, and layer blobs are the
  exact checked Nix OCI archive from the reviewed source revision, and prohibit
  entrypoint overrides, injected executables/libraries, or other
  code-affecting runtime substitutions.
- [ ] **Single-active coupled storage:** run exactly one active collector with
  exclusive access to one volume that contains its SQLite database, WAL/SHM,
  and `logs/`. Use neither a remote database, shared filesystem, object-store
  archive, rolling overlap, nor multi-active replicas.
- [ ] **Encrypted, qualified storage:** use an encrypted persistent volume and
  encrypted backups. Qualify the storage stack's destructive and failure
  behavior for this workload; the application key does not encrypt the archive.
- [ ] **Data-root provisioning:** provision the empty mount as
  `10001:10001`, mode `0700`, and verify that UID 10001 can create and fsync a
  file before registration ingress is enabled.
- [ ] **Secret and data custody:** deliver the 32-byte key read-only, outside
  the image and environment; limit live data and key access to the runtime
  service plus explicitly trusted provisioning/backup identities, and limit
  backup copies to explicitly trusted backup identities. Treat any identity
  that copies the source key or prepares an existing data volume as a key and
  data custodian.
- [ ] **Public registration boundary:** terminate TLS before the collector and
  forward only `/v1/telemetry/registrations` to port 8175. Keep
  `PUBLIC_BASE_URL` equal to that externally visible HTTPS origin, without a
  path or trailing slash.
- [ ] **Private-listener isolation:** never expose port 8176 through public
  ingress. Bind it to loopback or enforce a telemetry-network firewall or
  NetworkPolicy that admits only the Prometheus scraper and authorized probe
  source; prove the policy with negative reachability tests. Setting
  `PRIVATE_BIND_ISOLATED=true` asserts this control but does not provide it.
- [ ] **Protocol compatibility:** deploy a collector that supports the FMan
  telemetry ALPN `fedi/fman/guardian-telemetry/1`. Do not configure a FMan or
  Fedimint release allowlist: the collector retains every independently valid
  family from compatible targets and continues journal polling when metrics are
  absent or invalid.
- [ ] **Telemetry backend:** configure the Prometheus-compatible backend with
  `honor_timestamps: true` and `track_timestamps_staleness: true`; assign TSDB,
  WAL, staleness, remote-write, and Grafana-query ownership to that backend.
  Grafana must query the backend, never the collector's private endpoint.
- [ ] **Coupled recovery:** stop the collector; take SQLite, WAL/SHM, `logs/`,
  and the matching key/key ID from one recovery point; restore them together
  to a private empty volume; and admit traffic only after startup recovery and
  `/ready` succeed.
- [ ] **Capacity and response:** reserve filesystem headroom beyond the archive
  quota for SQLite and WAL, monitor free space and admission-discard/rejection
  signals, and use the documented poison/capacity recovery procedure. Do not
  treat a retention reduction as immediate free space.
- [ ] **Traffic and shutdown:** use `/health` for liveness and `/ready` for
  readiness, keep unready instances out of traffic, send SIGTERM, and configure
  at least a 75-second termination grace period.

The rest of this guide explains and operationalizes this contract. It does not
create a second deployment guide.

## Image and process contract

Build or load the image on Linux:

```console
nix build .#cloud-fman-telemetry-oci-image
nix run .#cloud-fman-telemetry-container-load
```

The image runs as numeric UID/GID `10001:10001`, exposes public registration on
8175 and private observability on 8176, and owns
`/var/lib/cloud-fman-telemetry`. Mount one encrypted persistent volume there.
Mount the 32-byte application key read-only at
`/run/secrets/cloud-fman-telemetry-key`; do not put it in an environment
variable or image layer. The runtime file must be readable only by the service
identity. If the platform cannot project that ownership directly, a separately
trusted provisioning identity may read the source secret only to create the
UID-10001 runtime copy. Restrict that identity and its immutable image as key
custodians; this source-secret boundary is not enforced by the collector. If
that identity also prepares an existing data volume, it can read the collector
state and archive and is a trusted data custodian too.

The image-layer directory is mode 0700 and owned by 10001, which also gives a
new Docker named volume that ownership. Kubernetes, host-path, and already
existing volumes replace the image inode. Provision the empty mount with exact
mode 0700 and ownership `10001:10001`; Kubernetes requires a privileged init
step or equivalent storage provisioning because `fsGroup` alone does not set
the directory owner or exact mode. Run the service with `runAsUser: 10001` and
`runAsGroup: 10001`, then verify both ownership/mode and that UID 10001 can
create and fsync a file there before enabling registration ingress. The daemon
intentionally does not start as root to repair volume ownership.

Use exactly one active container for a volume. Do not use a remote SQLite
database, shared filesystem, object-store archive, rolling overlap, or
multi-active replica set. Those arrangements break the archive-before-cursor
commit invariant.

Copy [`config.example.env`](../../packages/cloud-fman-telemetry/config.example.env)
and replace every placeholder. `cloud-fman-telemetry --help` is the
authoritative CLI and environment-variable schema. In particular:

- `PUBLIC_BASE_URL` is the externally visible HTTPS origin, with no path or
  trailing slash. Terminate TLS before the collector and forward only
  `/v1/telemetry/registrations` to port 8175.
- A non-loopback `PRIVATE_BIND` requires `PRIVATE_BIND_ISOLATED=true`. This is an
  explicit operator assertion, not an application check of NetworkPolicy.
- `ENVIRONMENT` selects credential verification policy; it does not select an
  FMan or Fedimint version. The telemetry ALPN is the wire-compatibility
  boundary. A future incompatible protocol needs a new negotiated ALPN/version,
  not a release-version setting.
- metrics cadence is exactly 900 or 1800 seconds. Production should begin at
  1800 seconds. Safe-journal cadence is independent and defaults to 300 seconds.
- the archive quota is compressed bytes and hard-fails new appends at the
  configured bound. Retention is by UTC reception day, not event timestamp.

The container healthcheck calls `GET /health`. Configure orchestration liveness
to the same path and readiness to `GET /ready` on port 8176. Both return 204
when healthy. Readiness reports local store and serving health; one unreachable
FMan does not make the service unready. Send SIGTERM and use a 75-second
orchestrator termination grace period to leave margin for listener drain, the current
60-second metrics target budget and final bounded durability work.

## Private Prometheus boundary

Port 8176 has no application-layer authentication. Bind it to loopback or an
access-controlled telemetry network. Network policy must admit only the
Prometheus scraper and health-probe source; never route it through public
ingress. Stable FMan and seat identities appear in its vetted metrics. The
collector refuses a non-loopback bind without the explicit isolation assertion,
but it cannot prove that the asserted deployment policy exists or works.

Prometheus must preserve the collector's observation timestamps:

```yaml
scrape_configs:
  - job_name: cloud-fman-telemetry
    scheme: http
    static_configs:
      - targets: ["cloud-fman-telemetry.telemetry.svc:8176"]
    metrics_path: /metrics
    scrape_interval: 30s
    honor_timestamps: true
    track_timestamps_staleness: true
```

The collector makes a deliberate observation at its configured 15- or
30-minute cadence and repeats its original timestamp between observations. A
30-second Prometheus scrape does not turn it into 30-second source data. Alert
on the exported stale/fresh metadata using the documented two-cadence
threshold. Grafana queries Prometheus (or its remote-write backend), never this
`/metrics` endpoint.

The collector exports
`cloud_fman_telemetry_metrics_admission_total{event=...}` with exactly five
events: `admitted`, `known_deny_discarded`, `unknown_discarded`,
`invalid_admitted_discarded`, and `rejected`. Discard events mean the collector
kept independently valid allowlisted families while omitting the affected
family; they do not expose the discarded family's name, labels, or values.
`target_fresh=1` therefore means the target completed a bounded safe projection,
not that every desired family was present or valid. Alert on increases in
`unknown_discarded`, `invalid_admitted_discarded`, or `rejected` and investigate
before changing the inventory. The collector also emits at most one fixed,
`safe_to_share` projection-degradation warning per five minutes; it never puts
the target, family, response, label, value, endpoint, or dependency error in that
diagnostic.

## Read-only staging comparison

This comparison audits the `k8s-devops` staging manifests at Git revision
`4fa3648421be814d126ebab029f093beac5921e8`, not a running cluster. It is
included to show how the known staging shape compares with this contract. It is
not evidence that Argo applied those manifests, that the cluster enforces them,
or that staging qualifies as production.

| Contract item | Staging-manifest observation | Result |
| --- | --- | --- |
| Artifact correspondence | `overlays/us1/kustomization.yaml` pins the collector by digest. The manifest cannot establish that the registry blobs are the checked Nix archive, that the image came from this revision, or that the live runtime has no substitution. | Partial; correspondence remains unknown. |
| Single-active coupled storage | The Deployment requests one replica, `Recreate`, and a RWO PVC; its data directory holds the coupled state. Live controller and storage behavior remain unknown. | Declared match; not live-verified. |
| Encrypted, qualified storage | `patch-cloud-fman-telemetry-pvc.yaml` selects `us-east-1a-ebs-sc`; the adjacent staging documentation says it is unencrypted and accepts that only for Mutinynet test data. | **Intentional staging relaxation; not production-compatible.** |
| Data-root provisioning and custody | A digest-pinned UID-0 init container sets `10001:10001`/`0700`, verifies fsync as UID 10001, and copies a read-only projected key to memory. The init identity is consequently a source-key/data custodian. Secret-manager, RBAC, and actual access controls are unknown. | Declared match; custody enforcement unknown. |
| Public registration boundary | A TLS Ingress routes only the exact registration path to the public Service. Certificate validity and live routing are unknown. | Declared match; live behavior unknown. |
| Private-listener isolation | The private Service has no Ingress, but the staging documentation says these clusters do not enforce NetworkPolicy and any pod can reach TCP 8176. | **Intentional staging relaxation; not production-compatible.** |
| Telemetry protocol compatibility | The manifest selects the collector image but cannot establish which live FMan releases support the collector's telemetry ALPN. Compatible targets are collected best-effort; an incompatible wire protocol cannot be inferred from release metadata. | Partial; live ALPN support unknown. |
| Prometheus backend | The ServiceMonitor preserves timestamps and staleness at 30 seconds. Backend WAL, remote write, Grafana query ownership, and actual scraping remain unknown. | Partial. |
| Coupled backup and restore | The staging collector README says to exercise coupled backup/restore before treating it as durable; the manifest audit found no evidence of a completed rehearsal or encrypted backup custody. | Unknown. |
| Capacity, traffic, and shutdown | The PVC requests 20 GiB for a 10 GiB archive quota; readiness/liveness use loopback and termination grace is 75 seconds. Free-space monitoring, capacity response, and live traffic gating remain unknown. | Declared partial match; operations unknown. |

The two intentional relaxations are acceptable only because this is staging
with Mutinynet test data. They must be removed and the unknowns evidenced
before a production deployment can rely on the production-readiness assessment.

## Archive inspection

Safe records live at:

```text
/var/lib/cloud-fman-telemetry/logs/<journal-stream>/<UTC-reception-day>.jsonl.zst
```

Each fetch is one independent zstd frame. A daily file is a valid concatenated
zstd stream. Decode all frames without rewriting the exact JSONL bytes, then
inspect the decoded copy separately:

```console
zstd -dc -- logs/STREAM/2026-08-23.jsonl.zst > archive.jsonl
jq -c . < archive.jsonl
```

Do not append, recompress, rename, or partially copy live files. Stream names
are opaque collector identifiers. Gap and source-incarnation metadata lives in
SQLite, not in fabricated JSONL records.

## Backup and restore

Treat `state.sqlite`, its WAL/SHM state, and `logs/` as one recovery unit.
Capabilities and registration material make the database and every backup
confidential.

1. Stop the single active collector and wait for clean exit.
2. Snapshot or copy the complete data directory from one recovery point.
3. Copy the application key and key identifier through the secret-management
   system, separately from the data backup.
4. Restore the complete directory and matching key to a private empty volume.
5. Start one collector with the same environment/profile and key identifier.
6. Require `/ready`, inspect startup logs for archive recovery failure, then
   re-enable registration ingress and Prometheus scraping.

Never restore SQLite without `logs/`, or `logs/` without SQLite. An
uncommitted archive tail is safe: startup verifies the final committed frame
hash and truncates later bytes. Missing or changed committed bytes are
indeterminate, and startup fails closed.

## Capacity, poison, and recovery

Monitor persistent-volume free space below the configured archive quota; SQLite
and WAL need additional headroom. Retention deletes complete UTC reception-day
files only after the ledger cutoff advances. Lowering retention does not make a
nearly full filesystem safe instantly.

Reaching the archive quota rejects each new nonempty journal batch before write
and leaves its source cursor unchanged for a later retry. The collector logs the
capacity refusal but stays ready and continues polling other targets. Because
the quota is shared, their nonempty journal batches are refused too until
retention deletes an older reception day or an operator increases capacity;
metrics collection and both HTTP listeners remain available.

An append or sync error poisons archive admission for the process because the
code cannot know how many bytes reached storage. Stop collection, diagnose the
volume, and restart from a coupled backup or from the unchanged volume. Startup
reconciles files against SQLite and clears poison only after recovery succeeds.
If it reports missing committed bytes or a committed-frame hash mismatch, do
not delete the row or file to force startup. Preserve both, isolate the volume,
restore the latest coupled backup, and re-register affected FMans if rollback
expired their leases.

## Registration rate and co-located fleets

The registration route admits a bounded number of requests per source network
prefix each minute, set by `CLOUD_FMAN_TELEMETRY_SOURCE_BUDGET` and defaulting
to `4`. Requests over the budget receive `503` and the FMan retries at its next
reconcile.

The default assumes one FMan per operator network, which is the deployment this
receiver is designed for. It does not fit a deployment that places several FMans
behind one egress address: the receiver sees one client, so the whole group
spends one budget. A reverse proxy in front of the collector changes which
address is counted — without `CLOUD_FMAN_TELEMETRY_TRUSTED_PROXY` the proxy's
own address is the source, and with it the forwarded address is. Neither can
separate FMans that share an egress.

Raise the budget where the deployment knowingly co-locates FMans, and size it
above the number of instances that share an address. Leave it at the default
otherwise: it is the only limit here bound to a scarce resource, and the
signature check in front of it does not stop an attacker who mints fresh keys.

## Capability and key rotation

A higher signed registration generation replaces the encrypted bearer
capability and fences in-flight work without resetting journal cursors. Never
edit capability ciphertext in SQLite. Ask the FMan operator to use its
owner-only telemetry re-enrollment/rotation path, then confirm a newer
registration before retiring the old capability.

The current collector deliberately has no in-place application-key re-encryption
command. Rotate the storage/KMS wrapping key while keeping the mounted 32-byte
data key stable. To replace that data key, deploy a fresh empty coupled volume
with a new key identifier, retain the old encrypted backup through its policy,
and require every FMan to register again. Do not start an existing database with
a different key or key identifier; startup rejects it. Archive bytes are not
encrypted by the application key, so encrypted-volume and backup encryption
remain mandatory.
