# Reconciliation

[Documentation](../README.md) · Decision: [ADR-0020](../adr/0020-reconciliation.md) · Step: [2.15](../roadmap/phase-2-operations-and-security/15-reconciliation.md)

Reconciliation finds every disagreement between the ledger and another
record of the same money, and never changes the ledger to remove one:

| Reconciliation | Compares | Run by |
|---|---|---|
| External | An account such as 1010 with the bank's statement | `mezgeb-cli recon import` and `recon run`, daily |
| Internal | A client's cached balances with the ledger's | The client, `POST /v1/reconciliation/balances` |
| Offline | Devices' reported counters and the escrows with the receipts and entries | `mezgeb-cli recon offline`, daily |

| File | What it is |
|---|---|
| [statement-format.md](statement-format.md) | Mezgeb's CSV layout, the mapping file for a bank's own export, and what clients record on entries so lines can match |
| [../runbooks/reconciliation.md](../runbooks/reconciliation.md) | The daily procedure, and what to do with each kind of break |
| [../accounting/entries/reconciliation.md](../accounting/entries/reconciliation.md) | The suspense entries that resolve money breaks (E32–E35) |
