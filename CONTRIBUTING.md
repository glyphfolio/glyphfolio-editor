# Contributing

Start with IMPLEMENTATION_PLAN.md and docs/DEPENDENCY_POLICY.md. Work in the library repository only; the current GlyphFolio application's migration and release work are independent.

Implement editor behavior originally. Low-level infrastructure is welcome within the documented boundary. Declare dependency/asset provenance and retain required notices. Contributions are intended under the project's MIT OR Apache-2.0 terms; no separate CLA or DCO workflow is imposed by this initial scaffold.

Include focused tests for behavior and invariants. Use public embedding tests to catch host coupling, property tests for transaction/history correctness, and native/synthetic evidence with explicit platform limits. Report NOT RUN, PASS, PARTIAL, or FAIL accurately; synthetic IME or AccessKit tests do not establish real OS interaction.

Before a change is ready for review, run `bash scripts/check-preview.sh` (Rust plus Python 3). It checks formatting, workspace tests, Clippy, docs, dependency boundaries and public-file hygiene. Minimize failing cases and keep fixture data synthetic or approved for public use. Preserve property-test failure seeds in reviewed fixtures rather than silently discarding regressions.

Public APIs are pre-1.0. Explain behavior changes, compatibility effects, and remaining limitations. Publishing a crate or adopting the engine in GlyphFolio requires separate release/adoption acceptance.
