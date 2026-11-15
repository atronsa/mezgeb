# 0017. Clients authenticate with OAuth 2.0 client credentials for opaque tokens; roles grant scopes from a permission matrix; a client touches only its own accounts

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-15 |
| Answers | Spec §13 (authentication between client apps and the ledger); TM-01, TM-05, TM-07, TM-10, TM-12, TM-13, TM-16 |
| Implemented in | 2.08 (authentication, authorization and privilege separation) |

## Context

The ledger's biggest risk is a request that looks legitimate but comes from
the wrong party: a compromised client server, or an insider with operator
access (threat model). Until step 2.08 the service read static bearer tokens
from a file. They never expired, had no revocation, and limited nothing to
the calling client's own accounts.

The contract (`openapi/mezgeb.v1.yaml`, ADR-0009) already names OAuth 2.0
client credentials with one scope per operation. What remained open was what
a token is, who holds which scopes, and which objects each caller may touch.

## Decision

1. **Clients and credentials.**
   - A *client* is a platform integrating with the ledger. It owns the
     accounts it opens, its idempotency keys and its rate limit.
   - A *credential* is one key for one *actor* of that client: a service, or
     a person on the operator's staff. It holds a set of *roles*.
   - The OAuth `client_id` is the credential's ID. Its secret is stored only
     as an Argon2id hash (`client_credentials`).
   - Credentials are created, listed and revoked with `mezgeb-cli client`,
     never over the API.
2. **Opaque access tokens.**
   - `POST /oauth/token` takes `grant_type=client_credentials` with HTTP
     Basic client authentication (RFC 6749 §4.4) and returns a random 256-bit
     token that lives 15 minutes.
   - The token is stored only as its SHA-256 (`access_tokens`), so every
     request costs one indexed lookup and nothing has to be signed.
   - A token is valid only while its credential and its client are active.
     Revoking either one ends every token at once.
   - Argon2 runs only at the token endpoint, never per request.
3. **Roles grant scopes, from a permission matrix kept as data.**
   - The matrix is `crates/api/permissions.toml`, built into the service, so
     a change to it is a reviewed code change. It lists six roles:

     | Role | Scopes |
     |---|---|
     | `poster` | open accounts, post through templates, place holds |
     | `reader` | read its own accounts, entries and holds |
     | `operator` | reversals, adjustment requests, period close |
     | `approver` | approve or reject adjustments |
     | `auditor` | read everything, reports |
     | `admin` | open system accounts; freeze, unfreeze and close accounts |

   - A role that grants `operations:write` lists the templates it may post.
     A template missing from every list, such as one released later, cannot
     be posted (default deny).
   - The matrix also names roles that one credential may never hold together
     (`operator` and `approver`: separation of duties).
4. **Authorization runs before anything else.**
   - Each handler declares the scopes its operation needs, as a type, in its
     first extractor. A caller without one of them gets `403 forbidden`
     before the body is read or the ledger touched.
   - There is no route without a declared scope: default deny.
   - A token may ask for fewer scopes than its roles grant (the `scope`
     parameter), never more.
5. **A client touches only its own accounts.**
   - **Accounts.** A client reads accounts it opened.
   - **Templates and holds.** These bind only accounts the client opened, or
     *system accounts* (chart kind `system`, opened by an admin) where the
     role allows their codes.
   - **Entries.** A client reads entries it posted, or that touch one of its
     accounts.
   - **Anything else is `404 not_found`, never `403`,** so IDs cannot be
     probed (TM-05).
   - **The scopes that act across clients** are `accounts:read_all`
     (auditor, operator), corrections (operator), adjustments
     (operator, approver) and account status (admin).
   - **Cross-client payments are not part of v1.** They would be a separate,
     explicit feature.
6. **Maker-checker stays in the engine and the database** (INV-25): the
   approver's actor must differ from the requester's. Separate credentials
   per person make "different actor" mean "different person".
7. **Closed periods are never reopened** (INV-10, ADR-0016). There is no
   reopen operation, so nothing needs a second approver. A correction of a
   closed period posts into the open one. TM-13 is met by that absence, which
   a test checks.
8. **Rate limits per client.**
   - Each client has a rate and a burst (`clients.rate_per_second`,
     `clients.burst`), enforced in process as a token bucket.
   - A request over the limit gets `429 rate_limited` with `Retry-After`.
   - The token endpoint is limited per credential, which slows down secret
     guessing as well as Argon2 does.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **OAuth client credentials, opaque tokens in the database (chosen)** | Revocation is instant; no signing key to protect or rotate; the contract's OAuth flow unchanged | One indexed lookup per request; tokens are only meaningful to this service |
| OAuth with signed JWTs (Ed25519) | No lookup per request; tokens verifiable elsewhere | A revoked credential's tokens live until they expire (or need a deny-list, which is the lookup again); a signing key to manage before step 3.12 has a key service |
| Hashed API keys on every request | Simplest | A slow hash per request (or a cache that delays revocation); long-lived secrets on every request; a contract change away from OAuth |
| Mutual TLS with pinned client certificates | Strong, no shared secrets | A certificate authority and rotation to run; awkward behind TLS-terminating proxies; the actor (person) still needs a second identity for maker-checker |
| Roles hard-coded in handlers | No data file | The role × operation × template matrix could not be listed, reviewed or tested as one thing |
| Linked clients that may post to each other's accounts | Cross-platform payments | Every ownership check gains a second path; not needed for v1 |
| Reopening a period with maker-checker | Fixes a closed period in place | Breaks INV-10: every report on that period could change; corrections in the open period serve the same need |

## Consequences

**Positive:**

- **TM-01, credentials stolen from a compromised client server.** The secret
  never travels with ordinary requests. A stolen token lasts at most
  15 minutes, and revoking the credential ends it at once. A stolen
  credential can reach only that client's accounts, within its roles and its
  rate limit.
- **TM-05 and TM-16, reading another client's data.** It is `404`, and only
  `accounts:read_all` reads across clients.
- **TM-07, posting outside the allowed templates.** Templates are granted per
  role, and an account can be bound only by its owner, or as a system account
  whose code the template role allows.
- **TM-12 and TM-13, insider corrections.** An adjustment needs two
  credentials of different people, and no one credential may hold both
  roles. There is no way to reopen a period.
- **The matrix is one reviewable file.** The authorization tests are
  generated from it and from the contract.

**Negative:**

- **Each request costs one token lookup.** An indexed primary-key read,
  acceptable next to a posting. Step 3.01 measures it.
- **Rate limits are per process.** Several instances each allow the full
  rate; a shared limiter comes with horizontal scaling (step 3.03).
- **No audit log yet.** Token issuance and refusals are logged, but they are
  not audit events until step 2.09.
- **Plain HTTP between proxy and service.** TLS is still terminated in front
  of the service.

**Makes out of date:** the interim token file (`MEZGEB_TOKENS`,
`auth/tokens.rs`, `deploy/tokens.dev.toml`), replaced by the clients tables
and `mezgeb-cli client`; spec §13's open item on authentication; TM-13's
"reopening needs maker-checker", which becomes "no reopen".
