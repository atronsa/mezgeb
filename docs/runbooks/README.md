# Runbooks

[Documentation](../README.md)

Step-by-step procedures for operators: what to run, what a good result looks
like, and what to do when it is not. Each runbook names the code it drives
and the decision behind it.

| Runbook | When |
|---|---|
| [close-period.md](close-period.md) | Daily, to close the accounting period (step 2.05) |
| [clients-and-credentials.md](clients-and-credentials.md) | Onboarding a client, issuing, rotating and revoking credentials, a leaked secret (step 2.08) |
| [offline.md](offline.md) | Setting up offline payments: the issuer key, a client's tiers; freezing and revoking devices; checking escrow (step 2.11) |
| [tamper-evidence.md](tamper-evidence.md) | After every close, to publish the checkpoint; daily, to verify the chains; on a finding (step 2.09) |
