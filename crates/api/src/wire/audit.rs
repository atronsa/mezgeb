//! Tamper evidence (ADR-0018): receipts, signing keys, checkpoints and
//! audit events.

use mezgeb_core::chain::hex;
use mezgeb_engine::{AuditEvent, Checkpoint, Receipt};
use serde::Serialize;

use super::format_time;

/// An entry's signed receipt (INV-26).
#[derive(Debug, Serialize)]
pub struct ReceiptJson {
    pub position: u64,
    pub entry: String,
    pub prev_hash: String,
    pub hash: String,
    pub key_id: Option<String>,
    pub signature: Option<String>,
}

impl From<&Receipt> for ReceiptJson {
    fn from(r: &Receipt) -> ReceiptJson {
        ReceiptJson {
            position: r.position,
            entry: r.entry.to_string(),
            prev_hash: hex(&r.prev_hash),
            hash: hex(&r.hash),
            key_id: r.key_id.clone(),
            signature: r.signature.as_deref().map(hex),
        }
    }
}

/// A public signing key.
#[derive(Debug, Serialize)]
pub struct SigningKeyJson {
    pub key_id: String,
    pub algorithm: &'static str,
    pub public_key: String,
}

/// A signed checkpoint.
#[derive(Debug, Serialize)]
pub struct CheckpointJson {
    pub id: i64,
    pub period_id: i64,
    pub position: u64,
    pub hash: String,
    pub audit_position: u64,
    pub audit_hash: String,
    pub at: String,
    pub key_id: String,
    pub signature: String,
    pub line: String,
}

impl From<&Checkpoint> for CheckpointJson {
    fn from(c: &Checkpoint) -> CheckpointJson {
        CheckpointJson {
            id: c.id,
            period_id: c.period,
            position: c.position,
            hash: hex(&c.hash),
            audit_position: c.audit_position,
            audit_hash: hex(&c.audit_hash),
            at: format_time(c.at),
            key_id: c.key_id.clone(),
            signature: hex(&c.signature),
            line: c.to_line(),
        }
    }
}

/// An audit event.
#[derive(Debug, Serialize)]
pub struct AuditEventJson {
    pub position: u64,
    pub at: String,
    pub client: Option<String>,
    pub actor: Option<String>,
    pub action: String,
    pub target: Option<String>,
    pub request_id: Option<String>,
    pub outcome: String,
    pub details: serde_json::Value,
    pub hash: String,
}

impl From<&AuditEvent> for AuditEventJson {
    fn from(a: &AuditEvent) -> AuditEventJson {
        let e = &a.event;
        AuditEventJson {
            position: a.position,
            at: format_time(a.at),
            client: e.client.map(|c| c.to_string()),
            actor: e.actor.clone(),
            action: e.action.clone(),
            target: e.target.clone(),
            request_id: e.request_id.clone(),
            outcome: e.outcome.clone(),
            details: serde_json::from_str(&e.details)
                .unwrap_or_else(|_| serde_json::json!({ "text": e.details })),
            hash: hex(&a.hash),
        }
    }
}
