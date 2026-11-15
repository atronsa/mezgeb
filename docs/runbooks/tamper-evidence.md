# Runbook: Tamper Evidence

[Runbooks](README.md) · Code: [`crates/engine/src/tamper.rs`](../../crates/engine/src/tamper.rs), [`crates/cli/src/chain.rs`](../../crates/cli/src/chain.rs) · Step: [2.09](../roadmap/phase-2-operations-and-security/09-audit-logging.md) · Decision: [ADR-0018](../adr/0018-tamper-evidence.md)

The ledger chains and signs every entry and audit event. Someone with full
database access could still rewrite history, but not without leaving
evidence, *if* the checkpoints are kept somewhere they cannot reach. This
runbook keeps them there and checks them.

| | |
|---|---|
| Who | The on-call operator |
| When | After every period close (daily); verification at least daily |
| Commands | `mezgeb-cli checkpoint export`, `mezgeb-cli verify --chain` |
| Connects as | `mezgeb_verifier` |

## The signing key

```sh
mezgeb-cli key generate --out /secure/path/mezgeb-signing.hex
```

- The service reads it from `MEZGEB_SIGNING_KEY_FILE`; so does
  `close-period`. Keep it like a password: readable only by those two.
- Never put it in the database, the repository or an environment file.
  Step 3.12 moves it to a key service.
- **Rotating:** generate a new key, point the service and `close-period` at
  it, restart. The old public key stays in `signing_keys`, so old receipts
  remain verifiable. Keep the old key file only as long as policy requires,
  then destroy it.
- **If the key leaks:** rotate at once, and treat receipts signed after the
  leak as unproven until the chains verify against checkpoints exported
  before it.

## After every close: publish the checkpoint

```sh
mezgeb-cli checkpoint export --dir /var/lib/mezgeb/checkpoints
```

Then copy that directory somewhere the database's administrators cannot
write: another server, write-once storage, or both. Clients can also fetch
the checkpoints themselves (`GET /v1/checkpoints`) and should keep the
`line` of each.

The export never overwrites a file. A `MISMATCH` line means the database
now holds a checkpoint different from the one exported earlier: **treat it
as an incident** (below).

## Daily: verify the chains

```sh
mezgeb-cli verify --chain --checkpoints /path/to/the/kept/copies
```

Exit status 0 is clean. Any `finding:` line is evidence of tampering or of a
defect: stop and investigate. A good result looks like:

```
chain verification: 104823 entries, 391207 audit events, 31 checkpoints: clean
```

## A finding

1. **Keep everything.** Do not repair or delete anything; copy the database
   (a snapshot) and the checkpoint copies.
2. **Read the finding.** It names what broke: an entry that no longer
   matches its hash (it or its lines were changed), missing positions
   (links deleted), an entry not in the chain (inserted directly), a
   signature that does not verify, or a checkpoint the chain no longer
   reaches (the tail was deleted or rewritten).
3. **Compare with the audit events** around that position and time, and
   with the database's own logs of superuser sessions.
4. **Restore from backup** to before the change (step 3.10), and compare
   the receipts clients hold for the affected entries: a client's receipt
   is proof of what the ledger recorded.
