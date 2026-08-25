# CLAIM-cloud-fman-telemetry-collector-oci-archive-declarations: The checked collector OCI archive declares its defaults

On Linux, successful evaluation of the explicitly invocable
`cloudFmanTelemetryOciImage` check establishes that the exact
`cloudFmanTelemetryContainerImage` archive selected by the repository release
aggregate declares the collector entrypoint, null command, numeric
`10001:10001` user, `/var/lib/cloud-fman-telemetry` working directory, two
ports, one data volume, healthcheck, OCI labels, and exactly nine checked
non-secret environment defaults. Its final root filesystem contains executable,
non-escaping entrypoint and healthcheck binaries and a real mode-`0700` image
data directory whose final-layer entry is owned by UID/GID 10001. The
configured publication workflow builds that aggregate and routes its loaded
collector member to the configured ECR repository; this is source-level wiring,
not evidence that publication ran or that a registry or deployment selected
the archive.

## Assumptions

- Nix, `dockerTools`, tar, Python and its `tarfile` module, jq, skopeo, umoci,
  and filesystem metadata faithfully implement the archive construction and
  check when invoked.
