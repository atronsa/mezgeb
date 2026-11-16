//! The offline entries, booked with their templates (ADR-0019): what the
//! engine's [`OfflineBooks`] asks for, drafted by `mezgeb-ops`.
//!
//! | Booking | Template | Worked entries |
//! |---|---|---|
//! | `settle` | `offline_receipt_settle.v1` | E17 |
//! | `pay` | `offline_receipt_pay.v1` | E26, E28, E29, E31 |
//! | `return_escrow` | `offline_voucher_return.v1` | E18, E25, E27 |
//! | `repay` | `offline_shortfall_repay.v1` | E30 |

use std::collections::BTreeMap;
use std::sync::Arc;

use mezgeb_core::{Account, AccountId, EntryDraft, Money};
use mezgeb_engine::{
    OfflineAccounts, OfflineBooks, PayRequest, RepayRequest, ReturnRequest, SettleRequest,
};
use mezgeb_ops::{OperationRequest, TemplateRegistry};
use sqlx::PgConnection;

use crate::wire::format_time;

/// Books the offline entries with the released templates.
#[derive(Clone)]
pub struct TemplateBooks {
    pub registry: Arc<TemplateRegistry>,
}

fn draft(
    registry: &TemplateRegistry,
    view: &BTreeMap<AccountId, Account>,
    template: &str,
    accounts: Vec<(&str, AccountId)>,
    amounts: Vec<(&str, Money)>,
    metadata: Vec<(&str, String)>,
    event_time: Option<mezgeb_core::Timestamp>,
) -> Result<EntryDraft, String> {
    let request = OperationRequest {
        template: template.into(),
        accounts: accounts
            .into_iter()
            .map(|(r, a)| (r.to_owned(), a))
            .collect(),
        amounts: amounts
            .into_iter()
            .filter(|(_, m)| m.is_positive())
            .map(|(n, m)| (n.to_owned(), m))
            .collect(),
        metadata: metadata
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        event_time,
        ..OperationRequest::default()
    };
    registry.draft(&request, view).map_err(|e| e.to_string())
}

fn receipt_metadata(
    voucher: String,
    device: String,
    counter: u64,
    claimed: Option<mezgeb_core::Timestamp>,
) -> Vec<(&'static str, String)> {
    let mut metadata = vec![
        ("voucher", voucher),
        ("device", device),
        ("counter", counter.to_string()),
    ];
    if let Some(claimed) = claimed {
        metadata.push(("claimed_event_time", format_time(claimed)));
    }
    metadata
}

impl OfflineBooks for TemplateBooks {
    fn settle(
        &self,
        view: &BTreeMap<AccountId, Account>,
        r: &SettleRequest,
    ) -> Result<EntryDraft, String> {
        let mut accounts = vec![("voucher_escrow", r.escrow), ("payee", r.payee)];
        if let Some(fees) = r.fee_account {
            accounts.push(("fee_revenue", fees));
        }
        draft(
            &self.registry,
            view,
            "offline_receipt_settle.v1",
            accounts,
            vec![("amount", r.amount), ("merchant_fee", r.fee)],
            receipt_metadata(
                r.voucher.to_string(),
                r.device.to_string(),
                r.counter,
                r.claimed_time,
            ),
            r.event_time,
        )
    }

    fn pay(
        &self,
        view: &BTreeMap<AccountId, Account>,
        r: &PayRequest,
    ) -> Result<EntryDraft, String> {
        let mut accounts = vec![("payee", r.payee)];
        for (role, account) in [
            ("voucher_escrow", r.escrow),
            ("payer_wallet", r.wallet),
            ("offline_loss", r.loss_account),
            ("fee_revenue", r.fee_account),
        ] {
            if let Some(account) = account {
                accounts.push((role, account));
            }
        }
        draft(
            &self.registry,
            view,
            "offline_receipt_pay.v1",
            accounts,
            vec![
                ("from_escrow", r.from_escrow),
                ("from_wallet", r.from_wallet),
                ("from_loss", r.from_loss),
                ("merchant_fee", r.fee),
            ],
            receipt_metadata(
                r.voucher.to_string(),
                r.device.to_string(),
                r.counter,
                r.claimed_time,
            ),
            r.event_time,
        )
    }

    fn return_escrow(
        &self,
        view: &BTreeMap<AccountId, Account>,
        r: &ReturnRequest,
    ) -> Result<EntryDraft, String> {
        draft(
            &self.registry,
            view,
            "offline_voucher_return.v1",
            vec![("voucher_escrow", r.escrow), ("customer_wallet", r.wallet)],
            vec![("amount", r.amount)],
            vec![
                ("voucher", r.voucher.to_string()),
                ("reason", r.reason.to_owned()),
            ],
            None,
        )
    }

    fn repay(
        &self,
        view: &BTreeMap<AccountId, Account>,
        r: &RepayRequest,
    ) -> Result<EntryDraft, String> {
        draft(
            &self.registry,
            view,
            "offline_shortfall_repay.v1",
            vec![
                ("customer_wallet", r.wallet),
                ("offline_loss", r.loss_account),
            ],
            vec![("amount", r.amount)],
            vec![("shortfall", r.shortfall.to_string())],
            None,
        )
    }
}

/// The ledger's fee (4010) and loss (5030) accounts, if they are open.
pub async fn offline_accounts(
    conn: &mut PgConnection,
) -> Result<OfflineAccounts, mezgeb_core::LedgerError> {
    Ok(OfflineAccounts {
        fee_account: mezgeb_engine::account_by_reference(conn, "4010")
            .await?
            .map(|a| a.id()),
        loss_account: mezgeb_engine::account_by_reference(conn, "5030")
            .await?
            .map(|a| a.id()),
    })
}

/// Runs the offline tick (voucher expiry, returns, gap timeouts, the late
/// deadline) every `every`, until the service stops. Each run is one call
/// to the engine's `offline_tick` at the current time; a failed run is
/// logged and the next one tries again.
pub async fn run_ticks(state: crate::AppState, every: std::time::Duration) {
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let state = state.clone();
        let run = state.tasks.clone().spawn(async move {
            let mut conn = state.pool.acquire().await.map_err(|e| e.to_string())?;
            let accounts = offline_accounts(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
            let books = TemplateBooks {
                registry: state.registry.clone(),
            };
            mezgeb_engine::offline_tick(
                &mut conn,
                crate::wire::now(),
                accounts,
                &books,
                &state.options,
            )
            .await
            .map_err(|e| e.to_string())
        });
        match run.await {
            Ok(Ok(report)) => {
                if !(report.graced.is_empty()
                    && report.returned.is_empty()
                    && report.ended.is_empty()
                    && report.settled_past_gaps.is_empty())
                {
                    tracing::info!(
                        graced = report.graced.len(),
                        returned = report.returned.len(),
                        ended = report.ended.len(),
                        past_gaps = report.settled_past_gaps.len(),
                        "offline tick"
                    );
                }
            }
            Ok(Err(error)) => tracing::error!(%error, "offline tick failed"),
            Err(error) => tracing::error!(%error, "offline tick stopped"),
        }
    }
}
