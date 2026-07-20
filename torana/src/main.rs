use clap::Parser;
use torana_core::{init_logging_from_config, load_config, Server};

#[derive(Parser)]
#[command(name = "torana")]
#[command(about = "Rust-native micro reverse proxy")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "torana.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    init_logging_from_config(&config)?;
    config.validate()?;

    Server::new(config).run(cli.config).await
}
