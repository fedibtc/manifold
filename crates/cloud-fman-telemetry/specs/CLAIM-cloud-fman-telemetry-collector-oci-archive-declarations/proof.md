# Proof: Checked cloud FMan telemetry collector OCI archive declarations

## Scope and model

Scope: `flake.nix`, `flake.lock`, `.github/workflows/publish.yml`,
`.github/scripts/load-single-docker-image`,
`.config/selfci/{ci.sh,ci.yaml}`,
`crates/cloud-fman-telemetry/{Cargo.toml,src/**}`,
`docs/telemetry/cloud-collector-testing.md`, and this claim and proof.

The model covers successful Linux evaluation of one explicitly invocable Nix
check and the exact archive derivation it inspects. “Declares” means an OCI
configuration value or a final-rootfs fact checked in that archive. Configured
workflow wiring means only that the checked source names the same derivation
and routes its loaded member toward ECR.

## Axioms

The archive-tooling assumption in
[the claim](../CLAIM-cloud-fman-telemetry-collector-oci-archive-declarations.md)
is trusted. Actual workflow execution, GitHub/ECR/IAM behavior, registry
persistence and contents, tag/digest identity, runtime overrides, secret
delivery, network policy, and deployment selection are outside the claim.

## Argument

1. **[test, code] Exact archive and process declaration.**
   `cloudFmanTelemetryContainerImageCheck` consumes
   `cloudFmanTelemetryContainerImage`, converts and unpacks it, and checks its
   entrypoint, null `Cmd`, `10001:10001` user, and working directory.
   `releaseContainerImages` selects that same derivation.
2. **[test] Final-rootfs executables and data directory.** The check rejects
   escaping or non-executable entrypoint and healthcheck paths in the merged
   rootfs. It requires the image-local data path to be a real mode-`0700`
   directory and verifies that its final-layer entry has UID/GID 10001.
3. **[test] Closed OCI metadata.** The check compares both exposed ports, the
   sole data-volume declaration, healthcheck, OCI labels, and the complete
   environment against exact expected values.
4. **[test, enum] Nine environment defaults.** The expected environment is
   public bind, private bind, data path, key-file path, metrics cadence, source
   version, source hash, journal cadence, and `SSL_CERT_FILE`. No other
   environment entry or application-secret value can satisfy the equality.
5. **[code] Configured publication route.** The publish workflow builds the
   release aggregate and passes its cloud-collector member to
   `load-single-docker-image`. That helper loads the passed archive, discovers
   and returns the sole tag reported for that loaded member. The workflow
   pushes the returned architecture tag to the collector ECR repository and
   includes it in the configured manifest. This establishes source
   correspondence only.
6. **[enum] Invocation boundary.** The check is exported on Linux but the
   required SelfCI script schedules neither collector contract derivation.
   Its guarantee is conditional on explicit successful invocation.

## Evidence boundary

The exact defaults bind the private listener to `0.0.0.0:8176` and omit
`CLOUD_FMAN_TELEMETRY_PRIVATE_BIND_ISOLATED=true`, public base URL,
environment, and key id. They deliberately do not form a standalone successful
production configuration.

## Residuals

Runtime user, entrypoint, environment, and mount overrides do not change what
the archive declares. Failed or absent publication, a different registry
object, and deployment by another digest do not contradict source-level wiring.

## Weakest links

Tool fidelity is axiomatic. The workflow route is `code`, and the exhaustive
environment categorization is `enum`; the archive facts themselves are
checked by the named derivation.
