//! Clients, credentials and access tokens (step 2.08, ADR-0017).
//!
//! A *client* is a platform: it owns the accounts it opens, its idempotency
//! keys and its rate limit. A *credential* is one key for one actor of a
//! client, with roles. Its ID is the OAuth `client_id`; its secret is kept
//! only as an Argon2id hash. An *access token* is kept only as its SHA-256,
//! and is valid only while it has not expired and its credential and its
//! client are active, so revoking either ends every token at once.
//!
//! Who calls what: `mezgeb-cli client`, as `mezgeb_admin`, creates and
//! revokes; the service, as `mezgeb_app`, logs credentials in and looks
//! tokens up, and cannot create a credential (migration 0009).

use std::fmt;
use std::fmt::Write as _;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use mezgeb_core::{ActorId, ClientId, LedgerError, Timestamp};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::rows::{constraint_violation, db, storage};

/// The roles a credential may hold (`crates/api/permissions.toml`).
pub const ROLES: [&str; 6] = [
    "poster", "reader", "operator", "approver", "auditor", "admin",
];

/// Roles one credential may never hold together: the same person would
/// both request and approve corrections (INV-25).
pub const EXCLUSIVE_ROLES: [(&str, &str); 1] = [("operator", "approver")];

/// What can go wrong managing clients and credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// A client with this name exists.
    NameTaken(String),
    /// No such client or credential.
    NotFound,
    /// A name, actor, role set or limit the database would refuse.
    Invalid(String),
    Ledger(LedgerError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::NameTaken(name) => write!(f, "a client named {name:?} exists"),
            ClientError::NotFound => f.write_str("no such client or credential"),
            ClientError::Invalid(why) => f.write_str(why),
            ClientError::Ledger(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<LedgerError> for ClientError {
    fn from(error: LedgerError) -> ClientError {
        ClientError::Ledger(error)
    }
}

fn sql(error: sqlx::Error) -> ClientError {
    ClientError::Ledger(db(error))
}

/// A client: a platform integrating with the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub id: ClientId,
    pub name: String,
    pub active: bool,
    pub rate_per_second: u32,
    pub burst: u32,
    pub created_at: Timestamp,
}

/// One key for one actor of a client. Its ID is the OAuth `client_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub id: Uuid,
    pub client: ClientId,
    pub actor: ActorId,
    pub roles: Vec<String>,
    pub created_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
}

/// Who an access token belongs to, and what it may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOwner {
    pub credential: Uuid,
    pub client: ClientId,
    pub actor: ActorId,
    /// The credential's roles, from which the service works out the
    /// templates it may post.
    pub roles: Vec<String>,
    /// The scopes granted to this token: at most what the roles allow.
    pub scopes: Vec<String>,
    pub rate_per_second: u32,
    pub burst: u32,
}

// ------------------------------------------------------------------ secrets

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn random_32() -> Result<[u8; 32], LedgerError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| storage(format!("no randomness: {e}")))?;
    Ok(bytes)
}

/// A new credential secret: 256 random bits, as `mzs_` and 64 hex digits.
/// It is shown once and stored only as [`hash_secret`].
pub fn generate_secret() -> Result<String, LedgerError> {
    Ok(format!("mzs_{}", hex(&random_32()?)))
}

/// The Argon2id hash of a secret, as a PHC string (OWASP parameters:
/// 19 MiB, 2 passes, 1 lane).
pub fn hash_secret(secret: &str) -> Result<String, LedgerError> {
    Argon2::default()
        .hash_password(secret.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| storage(format!("cannot hash the secret: {e}")))
}

/// Whether `secret` matches the stored hash. Argon2 is deliberately slow;
/// call it off the async runtime.
pub fn verify_secret(hash: &str, secret: &str) -> bool {
    Argon2::default()
        .verify_password(secret.as_bytes(), hash)
        .is_ok()
}

/// A new access token, as `mzt_` and 64 hex digits, with its SHA-256.
pub fn generate_token() -> Result<(String, [u8; 32]), LedgerError> {
    let token = format!("mzt_{}", hex(&random_32()?));
    let digest = token_digest(&token);
    Ok((token, digest))
}

/// The SHA-256 a token is stored as.
pub fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

// ------------------------------------------------------------------ rows

fn micros(row: &PgRow, column: &str) -> Result<Timestamp, LedgerError> {
    Ok(Timestamp::from_micros(row.try_get(column).map_err(db)?))
}

fn client_id(uuid: Uuid) -> Result<ClientId, LedgerError> {
    ClientId::from_uuid(uuid).map_err(storage)
}

fn limit(row: &PgRow, column: &str) -> Result<u32, LedgerError> {
    let value: i32 = row.try_get(column).map_err(db)?;
    u32::try_from(value).map_err(storage)
}

const CLIENT_COLUMNS: &str = "id, name, status, rate_per_second, burst, \
     (extract(epoch FROM created_at) * 1000000)::bigint AS created_micros";

fn client_from_row(row: &PgRow) -> Result<Client, LedgerError> {
    let status: String = row.try_get("status").map_err(db)?;
    Ok(Client {
        id: client_id(row.try_get("id").map_err(db)?)?,
        name: row.try_get("name").map_err(db)?,
        active: status == "active",
        rate_per_second: limit(row, "rate_per_second")?,
        burst: limit(row, "burst")?,
        created_at: micros(row, "created_micros")?,
    })
}

const CREDENTIAL_COLUMNS: &str = "id, client_id, actor, roles, \
     (extract(epoch FROM created_at) * 1000000)::bigint AS created_micros, \
     (extract(epoch FROM revoked_at) * 1000000)::bigint AS revoked_micros";

fn credential_from_row(row: &PgRow) -> Result<Credential, LedgerError> {
    let actor: String = row.try_get("actor").map_err(db)?;
    let revoked: Option<i64> = row.try_get("revoked_micros").map_err(db)?;
    Ok(Credential {
        id: row.try_get("id").map_err(db)?,
        client: client_id(row.try_get("client_id").map_err(db)?)?,
        actor: ActorId::new(actor).map_err(storage)?,
        roles: row.try_get("roles").map_err(db)?,
        created_at: micros(row, "created_micros")?,
        revoked_at: revoked.map(Timestamp::from_micros),
    })
}

// ------------------------------------------------------------------ admin

/// Checks a role set the way the database does, with a clearer message.
pub fn check_roles(roles: &[String]) -> Result<(), ClientError> {
    if roles.is_empty() {
        return Err(ClientError::Invalid(
            "a credential needs at least one role".into(),
        ));
    }
    if let Some(unknown) = roles.iter().find(|r| !ROLES.contains(&r.as_str())) {
        return Err(ClientError::Invalid(format!(
            "unknown role {unknown:?}; the roles are {}",
            ROLES.join(", ")
        )));
    }
    for (a, b) in EXCLUSIVE_ROLES {
        if roles.iter().any(|r| r == a) && roles.iter().any(|r| r == b) {
            return Err(ClientError::Invalid(format!(
                "one credential may not be both {a} and {b} (separation of duties)"
            )));
        }
    }
    Ok(())
}

/// Creates a client. As `mezgeb_admin`.
pub async fn create_client(
    conn: &mut PgConnection,
    id: ClientId,
    name: &str,
    rate_per_second: u32,
    burst: u32,
) -> Result<Client, ClientError> {
    let rate = i32::try_from(rate_per_second)
        .map_err(|_| ClientError::Invalid("rate too large".into()))?;
    let burst = i32::try_from(burst).map_err(|_| ClientError::Invalid("burst too large".into()))?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO clients (id, name, rate_per_second, burst) VALUES ($1, $2, $3, $4)
         RETURNING {CLIENT_COLUMNS}"
    )))
    .bind(id.as_uuid())
    .bind(name)
    .bind(rate)
    .bind(burst)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| match constraint_violation(&e) {
        Some((code, _)) if code == "23505" => ClientError::NameTaken(name.to_owned()),
        Some((code, constraint)) if code == "23514" => {
            ClientError::Invalid(format!("refused by {constraint}: the name is 1 to 64 of a-z 0-9 . _ - (starting with a letter or digit), the limits 1 to 100000"))
        }
        _ => sql(e),
    })?;
    Ok(client_from_row(&row)?)
}

/// The client with this name.
pub async fn client_by_name(
    conn: &mut PgConnection,
    name: &str,
) -> Result<Option<Client>, ClientError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {CLIENT_COLUMNS} FROM clients WHERE name = $1"
    )))
    .bind(name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(row.as_ref().map(client_from_row).transpose()?)
}

/// Every client, by name.
pub async fn list_clients(conn: &mut PgConnection) -> Result<Vec<Client>, ClientError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {CLIENT_COLUMNS} FROM clients ORDER BY name"
    )))
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(rows.iter().map(client_from_row).collect::<Result<_, _>>()?)
}

/// Disables a client: its tokens stop working at once and it cannot log in
/// again. A client is never enabled again (create a new one).
pub async fn disable_client(conn: &mut PgConnection, id: ClientId) -> Result<Client, ClientError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE clients SET status = 'disabled', disabled_at = clock_timestamp()
          WHERE id = $1 AND status = 'active' RETURNING {CLIENT_COLUMNS}"
    )))
    .bind(id.as_uuid())
    .fetch_optional(&mut *conn)
    .await
    .map_err(sql)?;
    match row {
        Some(row) => Ok(client_from_row(&row)?),
        None => {
            let existing = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {CLIENT_COLUMNS} FROM clients WHERE id = $1"
            )))
            .bind(id.as_uuid())
            .fetch_optional(&mut *conn)
            .await
            .map_err(sql)?;
            existing
                .as_ref()
                .map(client_from_row)
                .transpose()?
                .ok_or(ClientError::NotFound)
        }
    }
}

/// Adds a credential to a client, with the hash of its secret. As
/// `mezgeb_admin`.
pub async fn add_credential(
    conn: &mut PgConnection,
    id: Uuid,
    client: ClientId,
    actor: &ActorId,
    roles: &[String],
    secret_hash: &str,
) -> Result<Credential, ClientError> {
    check_roles(roles)?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO client_credentials (id, client_id, actor, roles, secret_hash)
         VALUES ($1, $2, $3, $4, $5) RETURNING {CREDENTIAL_COLUMNS}"
    )))
    .bind(id)
    .bind(client.as_uuid())
    .bind(actor.as_str())
    .bind(roles)
    .bind(secret_hash)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| match constraint_violation(&e) {
        Some((code, _)) if code == "23503" => ClientError::NotFound,
        _ => sql(e),
    })?;
    Ok(credential_from_row(&row)?)
}

/// A client's credentials, oldest first, revoked ones included.
pub async fn list_credentials(
    conn: &mut PgConnection,
    client: ClientId,
) -> Result<Vec<Credential>, ClientError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {CREDENTIAL_COLUMNS} FROM client_credentials WHERE client_id = $1
          ORDER BY created_at, id"
    )))
    .bind(client.as_uuid())
    .fetch_all(&mut *conn)
    .await
    .map_err(sql)?;
    Ok(rows
        .iter()
        .map(credential_from_row)
        .collect::<Result<_, _>>()?)
}

/// Revokes a credential: its tokens stop working at once. Revoking it again
/// returns it unchanged.
pub async fn revoke_credential(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Credential, ClientError> {
    sqlx::query(
        "UPDATE client_credentials SET revoked_at = clock_timestamp()
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(sql)?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {CREDENTIAL_COLUMNS} FROM client_credentials WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(sql)?;
    row.as_ref()
        .map(credential_from_row)
        .transpose()?
        .ok_or(ClientError::NotFound)
}

// ------------------------------------------------------------------ service

/// An active credential and its active client, with the stored secret
/// hash, for logging in. `None` if either is revoked or disabled, or the ID
/// is unknown: the caller answers all of these the same way.
pub async fn credential_for_login(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<(Credential, Client, String)>, LedgerError> {
    let row = sqlx::query(
        "SELECT c.id, c.client_id, c.actor, c.roles, c.secret_hash,
                (extract(epoch FROM c.created_at) * 1000000)::bigint AS created_micros,
                NULL::bigint AS revoked_micros,
                k.name, k.status, k.rate_per_second, k.burst,
                (extract(epoch FROM k.created_at) * 1000000)::bigint AS client_created_micros
           FROM client_credentials c JOIN clients k ON k.id = c.client_id
          WHERE c.id = $1 AND c.revoked_at IS NULL AND k.status = 'active'",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let credential = credential_from_row(&row)?;
    let client = Client {
        id: credential.client,
        name: row.try_get("name").map_err(db)?,
        active: true,
        rate_per_second: limit(&row, "rate_per_second")?,
        burst: limit(&row, "burst")?,
        created_at: micros(&row, "client_created_micros")?,
    };
    let hash: String = row.try_get("secret_hash").map_err(db)?;
    Ok(Some((credential, client, hash)))
}

/// Stores a new access token by its digest.
pub async fn store_token(
    conn: &mut PgConnection,
    digest: &[u8; 32],
    credential: Uuid,
    scopes: &[String],
    expires_at: Timestamp,
) -> Result<(), LedgerError> {
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, credential_id, scopes, expires_at)
         VALUES ($1, $2, $3, to_timestamp($4::double precision / 1000000))",
    )
    .bind(&digest[..])
    .bind(credential)
    .bind(scopes)
    .bind(expires_at.as_micros())
    .execute(&mut *conn)
    .await
    .map_err(db)?;
    Ok(())
}

/// The owner of a token, if it is known, unexpired, and its credential and
/// client are still active.
pub async fn token_owner(
    conn: &mut PgConnection,
    digest: &[u8; 32],
) -> Result<Option<TokenOwner>, LedgerError> {
    let row = sqlx::query(
        "SELECT c.id, c.client_id, c.actor, c.roles, t.scopes, k.rate_per_second, k.burst
           FROM access_tokens t
           JOIN client_credentials c ON c.id = t.credential_id
           JOIN clients k ON k.id = c.client_id
          WHERE t.token_hash = $1 AND t.expires_at > clock_timestamp()
            AND c.revoked_at IS NULL AND k.status = 'active'",
    )
    .bind(&digest[..])
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let actor: String = row.try_get("actor").map_err(db)?;
    Ok(Some(TokenOwner {
        credential: row.try_get("id").map_err(db)?,
        client: client_id(row.try_get("client_id").map_err(db)?)?,
        actor: ActorId::new(actor).map_err(storage)?,
        roles: row.try_get("roles").map_err(db)?,
        scopes: row.try_get("scopes").map_err(db)?,
        rate_per_second: limit(&row, "rate_per_second")?,
        burst: limit(&row, "burst")?,
    }))
}

/// Deletes tokens that expired before `before`; returns how many.
pub async fn purge_expired_tokens(
    conn: &mut PgConnection,
    before: Timestamp,
) -> Result<u64, LedgerError> {
    let done = sqlx::query(
        "DELETE FROM access_tokens WHERE expires_at < to_timestamp($1::double precision / 1000000)",
    )
    .bind(before.as_micros())
    .execute(&mut *conn)
    .await
    .map_err(db)?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_hash_and_verify() -> Result<(), LedgerError> {
        let secret = generate_secret()?;
        assert!(secret.starts_with("mzs_") && secret.len() == 68);
        let hash = hash_secret(&secret)?;
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{hash}"
        );
        assert!(verify_secret(&hash, &secret));
        assert!(!verify_secret(&hash, "mzs_wrong"));
        assert!(!verify_secret("not a hash", &secret));
        assert_ne!(generate_secret()?, secret);
        Ok(())
    }

    #[test]
    fn tokens_are_stored_by_their_digest() -> Result<(), LedgerError> {
        let (token, digest) = generate_token()?;
        assert!(token.starts_with("mzt_") && token.len() == 68);
        assert_eq!(token_digest(&token), digest);
        assert_ne!(generate_token()?.0, token);
        Ok(())
    }

    #[test]
    fn roles_are_checked() {
        let roles = |list: &[&str]| list.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>();
        assert!(check_roles(&roles(&["poster", "reader"])).is_ok());
        assert!(check_roles(&roles(&[])).is_err());
        assert!(check_roles(&roles(&["root"])).is_err());
        assert!(check_roles(&roles(&["operator", "approver"])).is_err());
    }
}
