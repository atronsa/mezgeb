# Runbook: Clients and Credentials

[Runbooks](README.md) · Code: [`crates/cli/src/clients.rs`](../../crates/cli/src/clients.rs), [`crates/engine/src/clients.rs`](../../crates/engine/src/clients.rs) · Step: [2.08](../roadmap/phase-2-operations-and-security/08-authentication-and-authorization.md) · Decision: [ADR-0017](../adr/0017-authentication-and-authorization.md)

Who may call the ledger, and what they may do. A *client* is a platform
that owns accounts; a *credential* is one key, for one actor of a client,
with roles from the [permission matrix](../../crates/api/permissions.toml).
Credentials are managed only from this command line: the API has no
endpoint for them, and the service's database role cannot create one.

| | |
|---|---|
| Who | The operator's administrator |
| Command | `mezgeb-cli client ...`, `mezgeb-cli credential ...` |
| Connects as | `mezgeb_admin` (`--database-url` or `MEZGEB_DATABASE_URL`) |
| Takes | Seconds; a revocation applies to the next request |

## Onboarding a client platform

```sh
mezgeb-cli client create wallet-app --rate 100 --burst 200
mezgeb-cli credential issue wallet-app --actor app:wallet --role poster --role reader
```

`credential issue` prints the `client_id` and the `client_secret` **once**.
Hand them to the client through its secret store, never by email or chat.
The client then gets tokens itself:

```sh
curl -u "$CLIENT_ID:$CLIENT_SECRET" -d grant_type=client_credentials \
    https://ledger.example/oauth/token
```

Give each credential the fewest roles it needs. A client platform's back
end is usually `poster` and `reader`; nothing a client platform does needs
`operator`, `approver`, `auditor` or `admin`.

## The operator's staff

Each person gets their own credential, so that maker-checker compares
people, not shared keys (INV-25):

```sh
mezgeb-cli client create operator --rate 20 --burst 40
mezgeb-cli credential issue operator --actor ops:alice --role operator
mezgeb-cli credential issue operator --actor ops:bekele --role approver
```

One credential is never both `operator` and `approver`; the command, the
service and the database each refuse it. The same person may hold an
`operator` credential and an `approver` credential, but cannot approve
their own adjustment request: the engine compares the actors.

## Rotating a secret

1. Issue a second credential for the same actor and roles.
2. Switch the client to the new `client_id` and secret.
3. Check the old credential is no longer used (the access log records the
   client; token issuance logs the credential).
4. Revoke the old one: `mezgeb-cli credential revoke <old client_id>`.

## A leaked secret or a compromised client server

Act first, investigate after:

1. `mezgeb-cli credential list <client>` to find the credential.
2. `mezgeb-cli credential revoke <client_id>`: its tokens stop on the next
   request, and it cannot get new ones.
3. If the client's server itself is compromised, disable the whole client:
   `mezgeb-cli client disable <client>`. Every credential and token of it
   stops. A disabled client is never enabled again; onboard a new one when
   the platform is clean.
4. Review what the client posted while compromised (its entries, by
   `client_id`), and correct with reversals or adjustments.

## Limits

`--rate` and `--burst` set the client's token bucket: `burst` requests at
once, refilled at `rate` per second. A client over it gets `429
rate_limited` with `Retry-After`. The limits are kept per service instance.

## Development and CI

`mezgeb-cli client seed deploy/clients.dev.toml` creates the development
clients with the secrets that file names, and skips those that exist.
Compose runs it after the migrations. Never seed that file, or any file of
known secrets, into a ledger holding real money.
