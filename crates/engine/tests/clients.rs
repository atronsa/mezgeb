//! Clients, credentials and access tokens on PostgreSQL (step 2.08,
//! ADR-0017): who may create them, what may change, and that revoking a
//! credential or disabling a client ends its tokens at once.

mod support;

use mezgeb_core::{ActorId, ClientId, Timestamp};
use mezgeb_engine::{
    ClientError, add_credential, client_by_name, create_client, credential_for_login,
    disable_client, generate_secret, generate_token, hash_secret, list_clients, list_credentials,
    purge_expired_tokens, revoke_credential, store_token, token_owner, verify_secret,
};
use support::{Server, TestResult};
use uuid::Uuid;

fn roles(list: &[&str]) -> Vec<String> {
    list.iter().map(|r| (*r).to_owned()).collect()
}

fn soon() -> Timestamp {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    Timestamp::from_micros(i64::try_from(now).unwrap_or(i64::MAX) + 900_000_000)
}

fn new_client_id() -> Result<ClientId, mezgeb_core::IdError> {
    ClientId::from_uuid(Uuid::now_v7())
}

#[tokio::test]
async fn credentials_log_in_and_tokens_end_with_them() -> TestResult {
    let Some(server) = Server::from_env()? else {
        return Ok(());
    };
    let db = server.migrated().await?;
    let mut admin = db.admin().await?;
    let mut app = db.app().await?;

    // The admin creates a client and a credential; names are unique.
    let wallet = create_client(&mut admin, new_client_id()?, "wallet-app", 50, 100).await?;
    assert!(wallet.active);
    assert_eq!((wallet.rate_per_second, wallet.burst), (50, 100));
    assert_eq!(
        create_client(&mut admin, new_client_id()?, "wallet-app", 1, 1).await,
        Err(ClientError::NameTaken("wallet-app".into()))
    );
    assert!(matches!(
        create_client(&mut admin, new_client_id()?, "Not A Name", 1, 1).await,
        Err(ClientError::Invalid(_))
    ));
    let secret = generate_secret()?;
    let credential = add_credential(
        &mut admin,
        Uuid::now_v7(),
        wallet.id,
        &ActorId::new("app:wallet")?,
        &roles(&["poster", "reader"]),
        &hash_secret(&secret)?,
    )
    .await?;
    assert_eq!(
        list_credentials(&mut admin, wallet.id).await?,
        vec![credential.clone()]
    );
    assert_eq!(
        client_by_name(&mut admin, "wallet-app").await?,
        Some(wallet.clone())
    );
    assert_eq!(list_clients(&mut admin).await?.len(), 1);
    // Separation of duties, in the engine and in the database.
    assert!(matches!(
        add_credential(
            &mut admin,
            Uuid::now_v7(),
            wallet.id,
            &ActorId::new("ops:both")?,
            &roles(&["operator", "approver"]),
            &hash_secret(&secret)?,
        )
        .await,
        Err(ClientError::Invalid(_))
    ));
    let both = sqlx::query(
        "INSERT INTO client_credentials (id, client_id, actor, roles, secret_hash)
         VALUES ($1, $2, 'ops:both', ARRAY['operator', 'approver'], '$argon2id$x')",
    )
    .bind(Uuid::now_v7())
    .bind(wallet.id.as_uuid())
    .execute(&mut admin)
    .await;
    assert!(
        both.is_err(),
        "the database refuses operator + approver too"
    );

    // The service logs the credential in with its secret, and issues a token.
    let (found, client, hash) = credential_for_login(&mut app, credential.id)
        .await?
        .ok_or("the credential cannot log in")?;
    assert_eq!((found.id, client.id), (credential.id, wallet.id));
    assert!(verify_secret(&hash, &secret));
    assert!(!verify_secret(&hash, "mzs_guess"));
    let (token, digest) = generate_token()?;
    store_token(
        &mut app,
        &digest,
        credential.id,
        &roles(&["accounts:read"]),
        soon(),
    )
    .await?;
    let owner = token_owner(&mut app, &digest)
        .await?
        .ok_or("the token is not found")?;
    assert_eq!(
        (owner.client, owner.actor.as_str(), owner.scopes.clone()),
        (wallet.id, "app:wallet", roles(&["accounts:read"]))
    );
    assert_eq!((owner.rate_per_second, owner.burst), (50, 100));
    assert!(token.starts_with("mzt_"));

    // The service cannot create, change or revoke credentials.
    let minted = add_credential(
        &mut app,
        Uuid::now_v7(),
        wallet.id,
        &ActorId::new("app:evil")?,
        &roles(&["admin"]),
        &hash_secret(&secret)?,
    )
    .await;
    assert!(minted.is_err(), "mezgeb_app must not create credentials");
    assert!(revoke_credential(&mut app, credential.id).await.is_err());
    assert!(
        sqlx::query("UPDATE clients SET rate_per_second = 100000")
            .execute(&mut app)
            .await
            .is_err()
    );
    // Nor can the admin change what a credential is.
    assert!(
        sqlx::query("UPDATE client_credentials SET roles = ARRAY['admin'] WHERE id = $1")
            .bind(credential.id)
            .execute(&mut admin)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM client_credentials")
            .execute(&mut admin)
            .await
            .is_err()
    );

    // Revoking the credential ends its token at once, and it cannot log in.
    let revoked = revoke_credential(&mut admin, credential.id).await?;
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoke_credential(&mut admin, credential.id).await?, revoked);
    assert_eq!(token_owner(&mut app, &digest).await?, None);
    assert!(
        credential_for_login(&mut app, credential.id)
            .await?
            .is_none()
    );
    let unrevoke = sqlx::query("UPDATE client_credentials SET revoked_at = NULL WHERE id = $1")
        .bind(credential.id)
        .execute(&mut admin)
        .await;
    assert!(unrevoke.is_err(), "a revoked credential stays revoked");

    // Disabling the client ends the tokens of all its credentials.
    let second = add_credential(
        &mut admin,
        Uuid::now_v7(),
        wallet.id,
        &ActorId::new("app:wallet-2")?,
        &roles(&["reader"]),
        &hash_secret(&secret)?,
    )
    .await?;
    let (_, digest) = generate_token()?;
    store_token(
        &mut app,
        &digest,
        second.id,
        &roles(&["accounts:read"]),
        soon(),
    )
    .await?;
    assert!(token_owner(&mut app, &digest).await?.is_some());
    let disabled = disable_client(&mut admin, wallet.id).await?;
    assert!(!disabled.active);
    assert_eq!(token_owner(&mut app, &digest).await?, None);
    assert!(credential_for_login(&mut app, second.id).await?.is_none());
    assert!(
        sqlx::query("UPDATE clients SET status = 'active', disabled_at = NULL WHERE id = $1")
            .bind(wallet.id.as_uuid())
            .execute(&mut admin)
            .await
            .is_err(),
        "a disabled client stays disabled"
    );

    // Expired tokens are not accepted, and are purged.
    let other = create_client(&mut admin, new_client_id()?, "other", 10, 10).await?;
    let third = add_credential(
        &mut admin,
        Uuid::now_v7(),
        other.id,
        &ActorId::new("app:other")?,
        &roles(&["reader"]),
        &hash_secret(&secret)?,
    )
    .await?;
    let (_, digest) = generate_token()?;
    store_token(
        &mut app,
        &digest,
        third.id,
        &roles(&["accounts:read"]),
        soon(),
    )
    .await?;
    sqlx::query("UPDATE access_tokens SET issued_at = now() - interval '2 hours', expires_at = now() - interval '1 hour' WHERE token_hash = $1")
        .bind(&digest[..])
        .execute(&mut db.superuser().await?)
        .await?;
    assert_eq!(token_owner(&mut app, &digest).await?, None);
    assert!(purge_expired_tokens(&mut app, soon()).await? >= 1);

    drop((admin, app));
    db.drop().await?;
    Ok(())
}
