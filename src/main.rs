use std::sync::Arc;

use tiebidli::{build_router, AppState, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cfg = Config::from_env()?;
    let db = tiebidli::db::Db::connect(&cfg.database_url, &cfg.prefix).await?;
    let listen = cfg.listen;
    let state = Arc::new(AppState::new(db, cfg));
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("Tiebidli 正在监听 {listen}");
    axum::serve(listener, app).await?;
    Ok(())
}
