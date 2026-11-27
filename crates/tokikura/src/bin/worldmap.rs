//! Vista previa del terreno: dibuja un mapa PNG del mundo (o de un bioma solo) a partir
//! de los archivos de `content/<paquete>/worldgen/`, sin abrir el juego.
//!
//! Con `--watch` vuelve a dibujarlo cada vez que cambia un archivo: se deja abierto el
//! PNG en un visor que se actualice solo y se editan los biomas viendo el resultado.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kuravoxel_common::icon;
use kuravoxel_server::biomes;
use kuravoxel_server::preview::{self, MapView};

const USAGE: &str = "usage: tokikura-worldmap [options]
  --out FILE          PNG to write (default: worldmap.png)
  --biome NAME        draw the whole world as that single biome
  --scale N           meters per pixel (default: 8)
  --center X,Z        center of the map (default: the whole world)
  --size W,H          size in pixels (with --center; default: 1024,1024)
  --tint              tint each area with its biome color
  --seed N            world seed (default: 1337)
  --content DIR       content folder (default: ./content)
  --watch             redraw whenever a worldgen file changes";

struct Options {
    out: PathBuf,
    content: PathBuf,
    biome: Option<String>,
    scale: i32,
    center: Option<[i32; 2]>,
    size: [u32; 2],
    tint: bool,
    seed: u32,
    watch: bool,
}

fn pair<T: std::str::FromStr>(text: &str) -> anyhow::Result<[T; 2]> {
    let parts: Vec<&str> = text.split(',').collect();
    match parts[..] {
        [a, b] => match (a.trim().parse(), b.trim().parse()) {
            (Ok(a), Ok(b)) => Ok([a, b]),
            _ => anyhow::bail!("'{text}' is not a pair of numbers"),
        },
        _ => anyhow::bail!("'{text}' must be two numbers separated by a comma"),
    }
}

fn draw(options: &Options) -> anyhow::Result<()> {
    let start = Instant::now();
    let (generator, blocks) = preview::open(&options.content, options.seed, options.biome.as_deref())?;
    let mut view = match options.center {
        Some(center) => MapView { center, scale: options.scale, size: options.size, tint_biomes: false },
        None => MapView::whole(generator.terrain(), options.scale),
    };
    view.tint_biomes = options.tint;
    let image = preview::render(&generator, &blocks, &view);
    std::fs::write(&options.out, icon::encode_png(&image)?)?;
    println!(
        "{}: {}x{} px at {} m/px in {:.1?}",
        options.out.display(),
        image.width,
        image.height,
        view.scale,
        start.elapsed()
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut options = Options {
        out: PathBuf::from("worldmap.png"),
        content: std::env::var_os("KURAVOXEL_CONTENT").map_or_else(|| PathBuf::from("content"), PathBuf::from),
        biome: None,
        scale: 8,
        center: None,
        size: [1024, 1024],
        tint: false,
        seed: 1337,
        watch: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow::anyhow!("missing value for {arg}"));
        match arg.as_str() {
            "--out" => options.out = value()?.into(),
            "--content" => options.content = value()?.into(),
            "--biome" => options.biome = Some(value()?),
            "--scale" => options.scale = value()?.parse::<i32>()?.clamp(1, 64),
            "--center" => options.center = Some(pair(&value()?)?),
            "--size" => options.size = pair::<u32>(&value()?)?.map(|side| side.clamp(16, 8192)),
            "--seed" => options.seed = value()?.parse()?,
            "--tint" => options.tint = true,
            "--watch" => options.watch = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}\n{USAGE}"),
        }
    }

    if !options.watch {
        return draw(&options);
    }
    // Un error en un archivo no cierra la herramienta: se avisa y se espera al arreglo.
    let mut seen = None;
    loop {
        let fingerprint = biomes::fingerprint(&options.content);
        if seen != Some(fingerprint) {
            seen = Some(fingerprint);
            if let Err(error) = draw(&options) {
                eprintln!("error: {error:#}");
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}
