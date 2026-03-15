use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use agent_voice::accounting::refresh_model_catalog_from_pricing_page;
use agent_voice::config::AppConfig;
use agent_voice::service::VoiceAgentService;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let cli = Cli::parse();
    let explicit_config = cli
        .config
        .or_else(|| std::env::var("AGENT_VOICE_CONFIG").ok().map(PathBuf::from));
    let config = match explicit_config {
        Some(path) => AppConfig::load(Some(&path), true)?,
        None => AppConfig::load(AppConfig::resolve_default_path().as_deref(), false)?,
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(config.logging.level.clone())),
        )
        .with_target(true)
        .compact()
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;

    if let Err(error) = refresh_model_catalog_from_pricing_page(&config.accounting).await {
        tracing::warn!(error = %error, "failed to refresh pricing catalog from OpenAI docs; using mounted catalog");
    }

    let service = VoiceAgentService::new(config).await?;
    service.run().await
}
