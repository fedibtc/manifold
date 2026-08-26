# Proof: Direct metric-admission confinement and bounds

## Scope and model

Scope: `crates/guardian-metrics-policy/src/lib.rs`,
`crates/cloud-fman-telemetry/src/metrics_policy.rs`, and
`crates/cloud-fman-telemetry/tests/metrics_policy.rs`.

The model quantifies over arbitrary response bytes and `MetricsIdentity`
arguments passed to one call of `MetricsPolicy::admit_until`. The observed
predicate is a successful `AdmittedMetrics` result returned by that call. It
does not quantify over `samples_json` loaded from SQLite, renderer output,
configuration-to-inventory correspondence, producer membership, observation
time, or subsequent storage.

## Axioms

The execution-integrity assumption in
[the claim](../CLAIM-cloud-fman-telemetry-fresh-metrics-admission-confined.md)
is trusted. All parsing, policy, identity insertion, and aggregate-bound
behavior is derived from the scoped code.

## Argument

1. **[code] Family disposition and admitted parsing fail closed.**
   `admit_until` decodes UTF-8, bounds line count and line size, parses each
   non-comment sample name, and sends it through `shape`. `shape` accepts only
   the explicit gauge, counter, and histogram families and enabled method
   families. An exact source-coded reviewed-deny counter name or histogram
   generated-series name is skipped; unknown names and suffix variants are also
   skipped. Skipped lines still consume line, sample, family, and deadline
   budgets. Every admitted line then passes full parsing, and `validate_labels`
   requires equality, not a subset, between input label keys and the selected
   shape.
2. **[test, code] Values must be finite and not sign-negative.**
   After parsing each value as `f64`, `admit_until` taints its family when
   `!value.is_finite()` or `value.is_sign_negative()`. The latter excludes
   ordinary negative values, `-0`, `-0.0`, and negative underflow such as
   `-1e-999`, even though the renderer retains the original numeric token.
   `signed_negative_values_discard_the_family_without_rewriting_valid_lexemes` pins
   those signed-zero, underflow, and non-finite cases.
3. **[code] Series and histogram structure is closed per family.**
   Before accepting a sample, `admit_until` taints its admitted family on a
   duplicate normalized series key and records each histogram's distinct bucket,
   sum, and count components. It stages rather than emits samples. Before
   returning, it discards a tainted family or one whose histogram lacks its exact
   sum, count, or bucket set, then emits every independently valid staged family.
4. **[code] Input identity keys cannot override inserted identity.**
   `ParsedSample` rejects malformed or duplicate input labels, and policy shape
   validation excludes `fman_id`, `fman_name`, `guardian_seat_id`, and
   `asserted_federation_id` from the input. `admit_until` then inserts exactly those four keys from the supplied
   `MetricsIdentity`; this function does not establish that those values are
   canonical.
5. **[code] Global failures are bounded and projection-local failures stay local.**
   `MAX_SAMPLES` rejects a 20,001st input sample. Invalid UTF-8, an empty family
   boundary, line/family/deadline exhaustion, and a missing or tainted required
   release-marker family reject the response. Other parsing, shape, label, value,
   duplicate, and histogram failures taint only the exact admitted family selected
   before parsing. Before adding a rendered
   sample, `MAX_OUTPUT_BYTES` rejects an aggregate over 2 MiB, including the
   four inserted identity labels. The line, family, and label limits further
   constrain work but are not needed for the two claimed aggregate bounds.
6. **[test] Hostile policy cases pin the principal rejection paths.**
   `unknown_and_invalid_families_do_not_suppress_an_unrelated_valid_family`,
   `complete_real_scrape_projects_exact_output_and_unknown_is_locally_discarded`,
   `missing_or_duplicate_release_fails_globally_but_incomplete_histogram_is_local`,
   and
   `malformed_and_hostile_cardinality_are_bounded` cover unknown shape and
   generated-suffix lookalikes, extra labels, identity override, malformed and
   duplicate input, incomplete histograms, and the exact hostile sample bound.
   `exact_inventory_adds_only_bounded_identity_labels` pins supplied identity
   insertion, including canonical rendering of `asserted_federation_id`; same-value and
   different-value producer collisions exercise the same family-local rejection.

## Residuals

Inventory completeness and correspondence, configured-peer membership,
persisted-row exposition, observation time, and renderer or storage behavior
are outside this direct-call claim. They are exclusions from its quantifiers,
not guarantees derived here.

## Weakest links

The complete source-coded `shape` dispatch, exact base/generated-family
grouping, family-local taint, normalized-series construction, histogram
completion, and aggregate accounting remain on the `code` rung. The
named tests exercise representative rejection paths rather than enumerating
every accepted family and shape. Execution integrity is an axiom.
