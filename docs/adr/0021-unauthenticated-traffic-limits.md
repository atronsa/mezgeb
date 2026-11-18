# 0021. Callers who have not authenticated are limited globally, optionally by address, and their refusals are audited in summary

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-18 |
| Answers | [ADR-0017](0017-authentication-and-authorization.md) §8 (rate limits); [ADR-0018](0018-tamper-evidence.md) (the audit chain); TM-03, TM-01 |
| Implemented in | 2.16 (security review) |

## Context

The security review of step 2.16 found that a caller with no valid
credential can make the service do expensive, lasting work:

- **The token endpoint is limited per credential ID only** (ADR-0017 §8).
  An attacker who sends a new random ID with each request gets a fresh
  bucket each time. Every such request costs one Argon2id hash, against the
  dummy hash that keeps unknown IDs as slow as wrong secrets. The CPU is
  spent before anything is refused.
- **Every refused request is an audit event.** A bad bearer token, an
  unknown route reached by `POST`, and every refused token request each
  append one row to `audit_events`. The table is append-only and hash
  chained (ADR-0018): its rows are never deleted, and each append takes the
  chain head's lock, so a flood of refusals also delays the audit events of
  real work.

The bearer token lookup itself is one indexed read, bounded by the
connection pool and the request timeout; it is not the problem.

A limit per source address would stop one attacker without touching anyone
else, but the service does not know the caller's real address when it
runs behind a proxy. Trusting `X-Forwarded-For` is safe only from a proxy
the operator controls, and Mezgeb does not choose the deployment.

## Decision

1. **Failed logins share one global bucket.** Failed token requests share a
   single bucket for the whole service: 20 per second, in bursts of 100.
   Each token request **reserves** one failure from it before the
   credential is looked up or any secret hashed, and gives it back if the
   secret is right. While the bucket is empty, the endpoint answers
   `429 slow_down` with `Retry-After`. Argon2 work by callers who do not
   know a secret is therefore bounded to that rate, however many IDs they
   invent and however many requests they send at once. The per-credential
   limit of ADR-0017 stays.
2. **An optional limit per address, off by default.**
   `MEZGEB_ADDRESS_FAILURE_LIMIT` (failures per second; `0`, the default,
   turns it off) limits failed logins and refused bearer tokens per source
   address, together, with a burst of ten times the rate. Each token
   request and each bearer token lookup reserves one from the address's
   bucket and gives it back on success, so an address's real requests never
   spend it. An address whose failures are spent is answered `429` before
   the lookup or the hash, whatever it sends. The address is the TCP peer,
   or, with `MEZGEB_CLIENT_ADDRESS=x-forwarded-for`, the last entry of
   `X-Forwarded-For`, the one the operator's own proxy added. Only an
   operator whose service is reachable through that proxy alone should set
   it.
3. **Why reserve first.** Checking the bucket first and counting the
   failure afterwards does not bound anything: a burst of concurrent
   attempts all pass the check before the first one has failed. The first
   implementation did exactly that, and the test that sends 400 logins at
   once found that all 400 were hashed.
4. **Refusals of unauthenticated callers are audited in summary.** A
   refused request with no authenticated caller (`401`, a refused token
   request, an unknown route, a `429` before authentication) is written as
   its own audit event only if it is the first of its kind in the minute:
   the first per actor tried (`credential:<id>`, or `anonymous`), and no
   more than 60 such events a minute in all. The others are counted, and
   once a minute, and at shutdown, one `refusals.suppressed` event (outcome
   `suppressed`, details `total` and `by_outcome`) records how many there
   were. Every refusal is still in the access log.
5. **Nothing changes for authenticated callers.** Their refusals (`403`,
   `422`, their own `429`) are still each an audit event: they are
   attributable, and already limited per client. A client over its own rate
   limit is now checked after the caller is known, so its `429` is audited
   under its own name rather than as anonymous.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **A global failure bucket, an optional address limit, summary audit events (chosen)** | Works however the service is deployed; bounds Argon2 work and audit growth by constants; nothing to trust by default | Under attack, real clients may wait to get a new token (their current tokens keep working) |
| A limit per address, always on | Stops one attacker without slowing anyone else | Behind a proxy every caller has the proxy's address, or the service must trust a header anyone can forge |
| A concurrency cap on Argon2 only | Bounds the CPU | The queue grows instead; audit rows are still one per refusal |
| Not auditing refusals of unauthenticated callers | No growth at all | Loses the evidence of an attack that TM-01 relies on; ADR-0018 asks for refusals |
| A proof-of-work or CAPTCHA at the token endpoint | Costs the attacker | Server-to-server clients; a contract change |

## Consequences

**Positive:**

- **TM-03.** An attacker without a secret causes at most 20 Argon2 hashes a
  second, and at most 61 audit events a minute, whatever the request rate.
- **TM-01.** Every refused login is still counted in the audit chain, and
  the first per credential each minute is recorded in full.

**Negative:**

- **A login flood delays real logins.** While the failure bucket is empty,
  a client whose token expires waits up to a second per attempt. Tokens
  live 15 minutes and clients renew them before they expire, so work goes
  on; an operator who can trust the proxy's address turns on the address
  limit to target the attacker instead.
- **Summaries are approximate in time.** A suppressed refusal is recorded
  up to a minute later, in a count, not with its request ID; the access
  log keeps each one.
- **The limits are per process**, like the others (step 3.03).
