mod app;
mod camera;
mod mesher;
mod net;
mod render;
mod world;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,voxel=info,voxel_server=info"),
    )
    .init();
    app::run()
}
