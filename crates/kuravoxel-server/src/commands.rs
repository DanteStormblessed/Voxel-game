//! Comandos de consola que ejecuta el servidor.
//!
//! Aquí solo se interpreta el texto; quién puede ejecutarlos y qué hacen lo decide
//! `Server::run_command`. Los comandos que solo afectan a un cliente (su cámara, sus
//! ajustes) no pasan por el servidor: los resuelve el juego en el propio cliente.

use glam::{DVec3, IVec3};
use kuravoxel_common::protocol::MAX_COMMAND_LENGTH;

/// Coordenada más lejana a la que se puede teletransportar a alguien.
const MAX_COORDINATE: f64 = 10_000_000.0;
/// Voxeles que puede cambiar un `fill` (un cubo de 64³).
pub const MAX_FILL: u64 = 64 * 64 * 64;

/// Ayuda de los comandos del servidor, una línea por comando.
pub const HELP: &[&str] = &[
    "tp <x> <y> <z>          teleport to a position (~ = relative to you)",
    "setblock <x> <y> <z> <block>   place a block, e.g. base:wood[axis=x]",
    "fill <x1> <y1> <z1> <x2> <y2> <z2> <block>   fill a box (air to clear it)",
    "seed                    show the world seed",
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
    Seed,
    Save,
    Players,
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
        "seed" => no_args(Command::Seed),
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
