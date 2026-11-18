//! The ledger REST service. Configured from the environment
//! ([`mezgeb_api::config`]); stops gracefully on Ctrl-C or SIGTERM,
//! finishing the requests and writes under way.

use std::process::ExitCode;
use std::sync::Arc;

use mezgeb_api::auth::Permissions;
use mezgeb_api::auth::rate::Limits;
use mezgeb_api::config::Config;
use mezgeb_api::{AppState, serve};
use mezgeb_core::Chart;
use mezgeb_engine::PostOptions;
use mezgeb_ops::TemplateRegistry;
use sqlx::postgres::PgPoolOptions;
use tokio_util::task::TaskTracker;

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down: finishing the requests under way");
}

async fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    let chart_text = std::fs::read_to_string(&config.chart)
        .map_err(|e| format!("cannot read {}: {e}", config.chart.display()))?;
    let chart = Chart::from_toml(&chart_text).map_err(|e| format!("chart: {e}"))?;
    let permissions = Permissions::built_in()?;
    let signer = mezgeb_engine::Signer::from_file(&config.signing_key)?;
    mezgeb_api::config::check_signing_key(signer.key_id())?;
    let pool = PgPoolOptions::new()
        .max_connections(config.pool_size)
        .acquire_timeout(config.request_timeout)
        .connect(&config.database_url)
        .await
        .map_err(|e| format!("cannot connect to the database: {e}"))?;
    // Publish the key's public half before signing anything with it.
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("cannot connect to the database: {e}"))?;
    signer
        .register(&mut conn)
        .await
        .map_err(|e| format!("cannot register the signing key: {e}"))?;
    let issuer = match &config.offline_issuer_key {
        Some(path) => {
            let issuer = mezgeb_engine::IssuerKey::from_file(path)?;
            issuer
                .register(&mut conn)
                .await
                .map_err(|e| format!("cannot register the offline issuer key: {e}"))?;
            tracing::info!(key_id = issuer.key_id(), "offline issuer key loaded");
            Some(Arc::new(issuer))
        }
        None => {
            tracing::info!("no offline issuer key: offline payments are off");
            None
        }
    };
    drop(conn);
    tracing::info!(key_id = signer.key_id(), "signing key loaded");
    let state = AppState {
        pool,
        chart: Arc::new(chart),
        permissions: Arc::new(permissions),
        token_ttl: config.token_ttl,
        limits: Arc::new(Limits::with_address_limit(config.address_limit)),
        refusals: Arc::default(),
        registry: Arc::new(TemplateRegistry::standard()),
        options: PostOptions {
            signer: Some(Arc::new(signer)),
            ..PostOptions::default()
        },
        issuer,
        tasks: TaskTracker::new(),
        request_timeout: config.request_timeout,
        body_limit: config.body_limit,
    };
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    tracing::info!(address = %config.listen, "mezgeb-api listening");
    if state.issuer.is_some() && !config.offline_tick.is_zero() {
        tokio::spawn(mezgeb_api::offline_books::run_ticks(
            state.clone(),
            config.offline_tick,
        ));
    }
    serve(listener, state, shutdown_signal())
        .await
        .map_err(|e| format!("server error: {e}"))?;
    tracing::info!("stopped");
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let filter = std::env::var("MEZGEB_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            tracing::error!(%message, "mezgeb-api could not start");
            eprintln!("mezgeb-api: {message}");
            ExitCode::from(2)
        }
    }
}
