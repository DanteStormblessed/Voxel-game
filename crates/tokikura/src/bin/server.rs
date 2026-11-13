//! Servidor dedicado.

use std::net::SocketAddr;

use voxel_common::protocol::DEFAULT_PORT;
use voxel_server::{quic, ServerConfig};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut bind: SocketAddr = format!("0.0.0.0:{DEFAULT_PORT}").parse()?;
    let mut config = ServerConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow::anyhow!("missing value for {arg}"));
        match arg.as_str() {
            "--bind" => bind = value()?.parse()?,
            "--seed" => config.seed = value()?.parse()?,
            "--content" => config.content_dir = value()?.into(),
            "--world" => config.world_dir = std::path::PathBuf::from("worlds").join(value()?),
            "--help" | "-h" => {
                println!("usage: voxel-server [--bind ADDRESS:PORT] [--seed N] [--world NAME] [--content DIR]");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    let handle = voxel_server::start(config)?;
    quic::listen(handle.clone(), bind)?;

    // El servidor vive en sus propios hilos; este espera a Ctrl+C para guardar y salir.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(tokio::signal::ctrl_c())?;
    log::info!("shutting down");
    handle.shutdown();
    Ok(())
}
