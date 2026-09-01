# Task 6c3d — minimum-notice proposal-create guard

## Scope and provenance

- Issue: #317
- Parent tracker: #180
- Feature: #138
- Prior finding: PR #232 discussion `r3889458385`
- Base: `683249e59f8b302bbf8dc7a1ba9a1f0ab1d076d4`
- Source head: `814b40d768cbc1f10913eb42e100e2eef2a99b6a`
- Runtime owner: `src/pa/service.rs`

The change adds no public API, schema, provider trait, dependency, deployment artifact, or owner-task behavior. It acts only on the new Google proposal-create path. Existing mappings and matching provider events continue through late-tail reconciliation without a duplicate create.

## Acceptance evidence

| Requirement | Evidence |
| --- | --- |
| Late new proposal is rejected before side effects | `submission_recheck_enforces_minimum_notice_at_create_boundary` passed and retained the prepared quote with zero provider creates, proposals, mappings, notifications, or audits. |
| Exact full-precision boundary is accepted | Dedicated equality regression passed and observed one Google proposal create. |
| Cutoff overflow is fail-closed | Dedicated regression passed with `Availability(DateTimeOverflow)`, a prepared quote, zero creates, and zero local rows. |
| Existing-event and mapping retries bypass the new-create guard | Existing matching-event and missing-tail selectors passed without duplicate provider creation. |
| Regression scope | Full `pa::service::tests` suite passed: 63 tests. |
| Repository gate | Fresh `rtk make check` passed after the complete source change: 582 tests. |
| Formatting and diff | Owned-file formatting and `rtk git diff --check` passed for the committed change. |

TDD RED was a real behavioral failure: the original implementation returned `SubmittedRequest` for a quote submitted four minutes inside the notice window. The same exact selector passed after the guard was added.

## Independent review

Independent read-only review found no correctness issues after the final regressions. It verified the guard is immediately before quote consumption and Google creation, uses untruncated UTC time, preserves equality, maps checked-add overflow correctly, and leaves late idempotent repair paths unchanged.

## Evidence boundaries

- LOCAL: PASS — focused selectors, full service suite, and repository gate.
- STATIC: PASS — one-file diff, independent review, and diff checks.
- CI: NOT RUN for this branch at report time.
- LIVE: NOT RUN — no provider, OAuth, SIP, calendar, email, deployment, or production action occurred.

## Risk and rollback

The behavioral risk is limited to external appointment proposals whose prepared start time has moved inside the configured minimum-notice window. Before merge, rollback is a normal revert of the one-file source commit and this one-file report commit. No external or persisted migration is involved.
