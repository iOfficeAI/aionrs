# Golden Body Snapshots

Baseline of outbound HTTP request bodies for the two wire dialects
(OpenAI-compatible, Anthropic-messages) across all current behavior branches.

**Purpose:** regression safety net for the ProviderCompat sub-struct split and
the TransportClient/RequestProjector/ResponseParser seam extraction. Any change
to these `.snap` files during a refactor MUST be intentional and reviewed via
`cargo insta review` — an unexpected diff means the refactor changed wire output.

**Scenarios:** see `docs/superpowers/plans/2026-06-25-golden-body-snapshot-baseline.md`
coverage matrix (13 scenarios across openai/anthropic/bedrock/vertex).

**Updating:** when a wire-output change is intended, run `cargo insta review`,
inspect each diff, accept only the intended ones, and commit the updated `.snap`
alongside the code change.
