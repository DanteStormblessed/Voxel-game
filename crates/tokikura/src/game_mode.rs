//! Modos de juego de Tokikura. El motor solo sabe cómo se mueve un cuerpo
//! (`MoveMode`); el nombre y el orden de los modos son cosa del juego.

use kuravoxel_common::physics::MoveMode;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GameMode {
    /// A pie, con colisiones y sin vuelo.
    #[default]
    Survival,
    /// Vuelo con colisiones.
    Creative,
    /// Vuelo sin colisiones.
    Spectator,
}

impl GameMode {
    pub const ALL: [GameMode; 3] = [GameMode::Survival, GameMode::Creative, GameMode::Spectator];

    pub fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "Survival",
            GameMode::Creative => "Creative",
            GameMode::Spectator => "Spectator",
        }
    }

    /// El siguiente modo, en el orden en que los recorre la tecla de cambio.
    pub fn next(self) -> GameMode {
        match self {
            GameMode::Survival => GameMode::Creative,
            GameMode::Creative => GameMode::Spectator,
            GameMode::Spectator => GameMode::Survival,
        }
    }

    pub fn move_mode(self) -> MoveMode {
        match self {
            GameMode::Survival => MoveMode::Walk,
            GameMode::Creative => MoveMode::Fly,
            GameMode::Spectator => MoveMode::Ghost,
        }
    }

    pub fn from_move_mode(mode: MoveMode) -> GameMode {
        match mode {
            MoveMode::Walk => GameMode::Survival,
            MoveMode::Fly => GameMode::Creative,
            MoveMode::Ghost => GameMode::Spectator,
        }
    }

    /// Por nombre, sin distinguir mayúsculas (para la consola).
    pub fn parse(name: &str) -> Option<GameMode> {
        Self::ALL.into_iter().find(|mode| mode.name().eq_ignore_ascii_case(name))
    }
}
