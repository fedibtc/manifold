# CLAIM-fleet-manager-guardian-metrics-egress-confined: Guardian metric egress is policy-confined

For arbitrary bytes returned by an authorized seat's loopback metrics endpoint,
an FMan Iroh metrics response contains only complete, independently valid
families selected by the exact compiled guardian source policy. Known-denied,
unknown, malformed, and incomplete families and families containing a duplicate
series do not appear in the response or response-derived diagnostics, and one
family-local failure does not suppress an unrelated valid family.

After a complete body is fetched, policy projection fails without a body only
when UTF-8 is invalid, the parser cannot isolate a family boundary, or a global
byte, line, sample, family, label, output, work, or deadline bound is exhausted.
An absent, duplicate, or invalid release-marker family is discarded locally.
Transport, metadata, read, or runtime failures also return no body. There is no
raw fallback.

The adversary controls every loopback response byte, header, status, chunk
boundary, metric name, label, and value after controlling or compromising a
seat child. It cannot modify the official FMan binary or the compiled policy.

## Assumptions

- Rust, Tokio, Reqwest, Iroh, and the operating system execute the reviewed
  official binary without memory corruption or code injection.
- The dedicated Iroh service serializes only the `GuardianMetricsResponse`
  returned by `GuardianTelemetryRpc`.
