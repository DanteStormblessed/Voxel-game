//! Comandos de consola que ejecuta el servidor.
//!
//! Aquí solo se interpreta el texto; quién puede ejecutarlos y qué hacen lo decide
//! `Server::run_command`. Los comandos que solo afectan a un cliente (su cámara, sus
//! ajustes) no pasan por el servidor: los resuelve el juego en el propio cliente.

use glam::{DVec3, IVec3};
use kuravoxel_common::protocol::{TerrainOp, MAX_COMMAND_LENGTH};

/// Coordenada más lejana a la que se puede teletransportar a alguien.
const MAX_COORDINATE: f64 = 10_000_000.0;
/// Voxeles que puede cambiar un `fill` (un cubo de 64³).
pub const MAX_FILL: u64 = 64 * 64 * 64;

/// Ayuda de los comandos del servidor, una línea por comando.
pub const HELP: &[&str] = &[
    "tp <x> <y> <z>          teleport to a position (~ = relative to you)",
    "setblock <x> <y> <z> <block>   place a block, e.g. base:wood[axis=x]",
    "fill <x1> <y1> <z1> <x2> <y2> <z2> <block>   fill a box (air to clear it)",
    "time                    show the time of day",
    "time set <hour>         set it: 0-24, 14:30, or dawn / noon / dusk / midnight",
    "time cycle <minutes>    real minutes a full day lasts (off or 0 stops time)",
    "seed                    show the world seed",
    "biome                   show the biome you are in and where the others are",
    "terrain                 show whether terrain changes reach the world on their own",
    "terrain auto on|off     regenerate the world as soon as the worldgen files change, or wait",
    "terrain apply           use the terrain files now (when live reload is off)",
    "terrain profiles        list the profiles of each biome",
    "terrain profile <biome> <name|default>   choose the profile a biome uses in this world",
    "save                    save the world now",
    "players                 list the connected players",
];

/// Coordenada de un comando: un número, o `~` / `~n` relativo a quien lo ejecuta.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coord {
    Absolute(f64),
    Relative(f64),
}

impl Coord {
    pub fn resolve(self, base: f64) -> f64 {
        match self {
            Coord::Absolute(value) => value,
            Coord::Relative(offset) => base + offset,
        }
    }
}

pub type Position = [Coord; 3];

/// Posición resuelta respecto a `base`.
pub fn resolve(pos: Position, base: DVec3) -> DVec3 {
    DVec3::new(pos[0].resolve(base.x), pos[1].resolve(base.y), pos[2].resolve(base.z))
}

/// Voxel de una posición resuelta respecto a `base`.
pub fn resolve_voxel(pos: Position, base: DVec3) -> IVec3 {
    resolve(pos, base).floor().as_ivec3()
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Teleport(Position),
    /// Poner un bloque, por nombre de estado (`base:stone`, `base:wood[axis=x]`, `air`).
    SetBlock(Position, String),
    /// Rellenar una caja (ambas esquinas incluidas).
    Fill(Position, Position, String),
    /// Decir la hora del mundo.
    Time,
    /// Poner la hora del día (0-24).
    TimeSet(f32),
    /// Duración del día en segundos reales (0 = parar el tiempo).
    TimeCycle(f32),
    /// Poner el día del mundo (cambia la fase de la luna).
    TimeDay(u32),
    /// Saltar a la próxima noche con esa fase de la luna (octavos: 0 = nueva, 4 = llena).
    TimeMoon(u32),
    Seed,
    /// Decir en qué bioma está el jugador y dónde quedan los demás.
    Biome,
    /// Recarga del terreno y perfiles de los biomas.
    Terrain(TerrainOp),
    /// Listar los perfiles de cada bioma.
    TerrainProfiles,
    Save,
    Players,
}

/// Hora del día escrita como número (`14.5`), reloj (`14:30`) o nombre.
fn time_of_day(text: &str) -> Result<f32, String> {
    let invalid = || format!("'{text}' is not a time. Use 0-24, 14:30, dawn, noon, dusk or midnight.");
    let hours = match text {
        "dawn" | "sunrise" => 6.0,
        "morning" => 9.0,
        "noon" | "day" => 12.0,
        "dusk" | "sunset" => 18.0,
        "night" => 21.0,
        "midnight" => 0.0,
        _ => match text.split_once(':') {
            Some((hours, minutes)) => {
                let hours: f32 = hours.parse().map_err(|_| invalid())?;
                let minutes: f32 = minutes.parse().map_err(|_| invalid())?;
                if !(0.0..60.0).contains(&minutes) {
                    return Err(invalid());
                }
                hours + minutes / 60.0
            }
            None => text.parse().map_err(|_| invalid())?,
        },
    };
    if hours.is_finite() && (0.0..=24.0).contains(&hours) {
        Ok(hours)
    } else {
        Err(invalid())
    }
}

fn coordinate(text: &str) -> Result<Coord, String> {
    let invalid = || format!("'{text}' is not a valid coordinate.");
    let (relative, number) = match text.strip_prefix('~') {
        Some("") => return Ok(Coord::Relative(0.0)),
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let value = number
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && value.abs() <= MAX_COORDINATE)
        .ok_or_else(invalid)?;
    Ok(if relative { Coord::Relative(value) } else { Coord::Absolute(value) })
}

fn position(args: &[&str]) -> Result<Position, String> {
    Ok([coordinate(args[0])?, coordinate(args[1])?, coordinate(args[2])?])
}

/// Interpreta una línea de consola. El error es el mensaje para quien la escribió.
pub fn parse(line: &str) -> Result<Command, String> {
    if line.len() > MAX_COMMAND_LENGTH {
        return Err("That command is too long.".into());
    }
    let mut words = line.split_whitespace();
    let name = words.next().ok_or("Empty command.")?;
    let args: Vec<&str> = words.collect();
    let no_args = |command: Command| {
        if args.is_empty() {
            Ok(command)
        } else {
            Err(format!("'{name}' takes no arguments."))
        }
    };
    match name {
        "help" => no_args(Command::Help),
        "time" => match args[..] {
            [] => Ok(Command::Time),
            ["set", value] => Ok(Command::TimeSet(time_of_day(value)?)),
            ["cycle", "off" | "stop"] => Ok(Command::TimeCycle(0.0)),
            ["cycle", minutes] => match minutes.parse::<f32>() {
                Ok(minutes) if minutes.is_finite() && minutes >= 0.0 => Ok(Command::TimeCycle(minutes * 60.0)),
                _ => Err(format!("'{minutes}' is not a number of minutes.")),
            },
            ["day", day] => day.parse().map(Command::TimeDay).map_err(|_| format!("'{day}' is not a day number.")),
            ["moon", phase] => moon_phase(phase).map(Command::TimeMoon),
            _ => Err("Usage: time, time set <hour>, time cycle <minutes|off>, time day <n> or time moon <phase>".into()),
        },
        "seed" => no_args(Command::Seed),
        "biome" | "biomes" => no_args(Command::Biome),
        "terrain" => match args[..] {
            [] => Ok(Command::Terrain(TerrainOp::Status)),
            ["auto", "on"] => Ok(Command::Terrain(TerrainOp::Auto(true))),
            ["auto", "off"] => Ok(Command::Terrain(TerrainOp::Auto(false))),
            ["apply"] => Ok(Command::Terrain(TerrainOp::Apply)),
            ["profiles"] => Ok(Command::TerrainProfiles),
            ["profile", biome, "default"] => {
                Ok(Command::Terrain(TerrainOp::Profile { biome: biome.into(), profile: None }))
            }
            ["profile", biome, profile] => {
                Ok(Command::Terrain(TerrainOp::Profile { biome: biome.into(), profile: Some(profile.into()) }))
            }
            _ => Err("Usage: terrain, terrain auto on|off, terrain apply, terrain profiles or terrain profile <biome> <name|default>".into()),
        },
        "save" => no_args(Command::Save),
        "players" => no_args(Command::Players),
        "tp" => match args[..] {
            [_, _, _] => Ok(Command::Teleport(position(&args)?)),
            _ => Err("Usage: tp <x> <y> <z>".into()),
        },
        "setblock" => match args[..] {
            [_, _, _, block] => Ok(Command::SetBlock(position(&args)?, block.to_string())),
            _ => Err("Usage: setblock <x> <y> <z> <block>".into()),
        },
        "fill" => match args[..] {
            [_, _, _, _, _, _, block] => {
                Ok(Command::Fill(position(&args[..3])?, position(&args[3..6])?, block.to_string()))
            }
            _ => Err("Usage: fill <x1> <y1> <z1> <x2> <y2> <z2> <block>".into()),
        },
        other => Err(format!("Unknown command '{other}'. Type 'help' for a list.")),
    }
}

/// Una fase de la luna por nombre (`new`, `full`, `first_quarter`, `waxing_gibbous`...),
/// en octavos del ciclo a partir de la luna nueva.
fn moon_phase(name: &str) -> Result<u32, String> {
    let wanted = name.trim().to_lowercase().replace(['_', '-'], " ");
    kuravoxel_common::protocol::MOON_PHASES
        .iter()
        .position(|phase| *phase == wanted || phase.trim_end_matches(" moon") == wanted)
        .map(|index| index as u32)
        .ok_or_else(|| "Phases: new, waxing_crescent, first_quarter, waxing_gibbous, full, waning_gibbous, last_quarter, waning_crescent".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_known_commands() {
        assert_eq!(parse("seed"), Ok(Command::Seed));
        assert_eq!(parse("  save  "), Ok(Command::Save));
        assert_eq!(parse("players"), Ok(Command::Players));
        assert_eq!(parse("help"), Ok(Command::Help));
        let abs = Coord::Absolute;
        assert_eq!(parse("tp 1 -20.5 300"), Ok(Command::Teleport([abs(1.0), abs(-20.5), abs(300.0)])));
        assert_eq!(
            parse("tp ~ ~10 ~-2.5"),
            Ok(Command::Teleport([Coord::Relative(0.0), Coord::Relative(10.0), Coord::Relative(-2.5)]))
        );
        assert_eq!(
            parse("setblock 1 2 3 base:wood[axis=x]"),
            Ok(Command::SetBlock([abs(1.0), abs(2.0), abs(3.0)], "base:wood[axis=x]".into()))
        );
        assert!(matches!(parse("fill 0 0 0 ~ ~ ~ air"), Ok(Command::Fill(_, _, block)) if block == "air"));
        assert_eq!(parse("time"), Ok(Command::Time));
        assert_eq!(parse("time set 14:30"), Ok(Command::TimeSet(14.5)));
        assert_eq!(parse("time set dusk"), Ok(Command::TimeSet(18.0)));
        assert_eq!(parse("time cycle 5"), Ok(Command::TimeCycle(300.0)));
        assert_eq!(parse("time cycle off"), Ok(Command::TimeCycle(0.0)));
        assert!(parse("time set 25").is_err());
        assert!(parse("time set 10:75").is_err());
        assert!(parse("time warp").is_err());
        assert_eq!(parse("time day 12"), Ok(Command::TimeDay(12)));
        assert_eq!(parse("time moon new"), Ok(Command::TimeMoon(0)));
        assert_eq!(parse("time moon full"), Ok(Command::TimeMoon(4)));
        assert_eq!(parse("time moon waning_crescent"), Ok(Command::TimeMoon(7)));
        assert!(parse("time moon blue").is_err());
        assert_eq!(parse("terrain auto off"), Ok(Command::Terrain(TerrainOp::Auto(false))));
        assert_eq!(parse("terrain apply"), Ok(Command::Terrain(TerrainOp::Apply)));
        assert_eq!(
            parse("terrain profile plains rivers"),
            Ok(Command::Terrain(TerrainOp::Profile { biome: "plains".into(), profile: Some("rivers".into()) }))
        );
        assert_eq!(
            parse("terrain profile plains default"),
            Ok(Command::Terrain(TerrainOp::Profile { biome: "plains".into(), profile: None }))
        );
        assert!(parse("terrain auto maybe").is_err());
    }

    #[test]
    fn rejects_what_it_does_not_understand() {
        assert!(parse("").is_err());
        assert!(parse("fly").unwrap_err().contains("Unknown command 'fly'"));
        assert!(parse("seed now").is_err());
        assert_eq!(parse("tp 1 2"), Err("Usage: tp <x> <y> <z>".into()));
        assert!(parse("tp 1 nan 3").is_err());
        assert!(parse("tp 1 2 1e30").is_err());
        assert!(parse(&"x".repeat(MAX_COMMAND_LENGTH + 1)).is_err());
        assert!(parse("tp ~x 0 0").is_err());
        assert!(parse("setblock 1 2 3").is_err());
        assert!(parse("fill 1 2 3 4 5 6").is_err());
    }

    #[test]
    fn relative_coordinates_resolve_from_the_player() {
        let pos = [Coord::Relative(1.0), Coord::Absolute(5.0), Coord::Relative(-0.5)];
        assert_eq!(resolve_voxel(pos, DVec3::new(10.2, 80.0, -3.2)), IVec3::new(11, 5, -4));
    }
}
