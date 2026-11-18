# Security

[Documentation](../README.md) · Threats: [threat-model.md](../threat-model.md) · Step: [2.16](../roadmap/phase-2-operations-and-security/16-phase-2-security-review-and-exit.md)

How Mezgeb's security is checked, and what was found. The threats it
defends against are in the [threat model](../threat-model.md); the API's
review against the OWASP API Security Top 10 is in
[api/owasp-review.md](../api/owasp-review.md).

| File | What it is |
|---|---|
| [asvs.md](asvs.md) | The OWASP ASVS 5.0 Level 2 self-assessment: every chapter, met or not, with its evidence |
| [findings.md](findings.md) | Every finding of the security review, with its severity and status |

## How it stays checked

| Check | Where | When |
|---|---|---|
| Authorization matrix: every operation × every role | `crates/api/tests/authz.rs` | Every change |
| Business-logic abuse (AC-01 to AC-09) | `crates/api/tests/abuse.rs` | Every change |
| API fuzzing | `crates/api/tests/fuzz.rs` | Every change (1,500 cases), nightly (50,000, a new seed) in `.github/workflows/fuzz.yml` |
| Threats through the API | `crates/api/tests/threats.rs` | Every change |
| Dependencies: advisories, licences, sources | cargo-deny (`deny.toml`) and cargo-audit (`.cargo/audit.toml`) in `.github/workflows/deps.yml` | Every change, and daily |

Report a vulnerability as [SECURITY.md](../../SECURITY.md) says.
