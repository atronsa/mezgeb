# Security Policy

Mezgeb is a ledger: its job is to hold the truth about money. A bug that
lets money be created, lost, double-spent or silently changed is a security
issue, even when no attacker is involved.

## Supported versions

Mezgeb is in early development and is **not production ready**. Only the
latest commit on `main` is supported. Fixes are not backported to earlier
tags.

| Version | Supported |
|---|---|
| `main` | Yes |
| Tags before 1.0 (such as `v0.1.0-engine`) | No |

## Reporting a vulnerability

**Please do not report security issues in public issues, pull requests or
discussions.**

Report privately, by either:

- **GitHub:** [report a vulnerability](https://github.com/atronsa/mezgeb/security/advisories/new)
  (Security → Advisories → Report a vulnerability), or
- **Email:** dagimcoder@gmail.com, with "Mezgeb security" in the subject.

Please include:

- what an attacker or a fault can do, and to which money or data
- the steps or code to reproduce it, and the commit you tested
- whether it breaks an invariant from [docs/invariants.md](docs/invariants.md),
  and which one, if you know

## What happens next

| When | What |
|---|---|
| Within 3 working days | We confirm we received the report |
| Within 10 working days | We tell you whether we accept it as a vulnerability, and how severe we think it is |
| When fixed | We publish a GitHub security advisory and credit you, unless you prefer not to be named |

We ask you to keep the issue private until a fix is released, or for 90
days from the report, whichever comes first. If a fix needs longer, we will
explain why and agree a new date with you.

## What counts

In scope, for example:

- any way to break an invariant in [docs/invariants.md](docs/invariants.md),
  such as an unbalanced entry, a balance that differs from its lines, an
  idempotency key applied twice, a hold resolved twice, or a changed or
  deleted entry
- any way for the service's database role (`mezgeb_app`) to change history,
  or for the read-only roles to write
- a verification run (`mezgeb-cli verify`) that reports clean on a ledger
  that is not
- crashes or lockups that a client can trigger and that leave the ledger
  inconsistent

Out of scope:

- the local development passwords in `db/init/` and `compose.yaml`, which
  are public on purpose and never used in production
- findings that need the PostgreSQL superuser or the `mezgeb_owner` role:
  those roles can change anything, by design
- denial of service by sheer volume, unless a small input causes it
