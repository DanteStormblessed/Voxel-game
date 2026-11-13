//! Comandos de la consola de desarrollador que resuelve el propio cliente.
//!
//! Aquí solo se interpreta el texto; `game.rs` los ejecuta. Lo que no es un comando del
//! cliente se envía tal cual al servidor (`tp`, `seed`...), que tiene los suyos.

/// Ayuda de los comandos del cliente, una línea por comando.
pub const HELP: &[&str] = &[
    "help            show this list",
    "clear           clear the console",
    "pos             show the camera position",
    "stats           show fps and world counters",
    "blocks [text]   list the blocks (and their states) matching the text",
    "items [text]    list the items matching the text",
    "give <item> [slot]  put an item in the hotbar (first free slot, or slot 1-9)",
    "block <item>    put an item in the selected hotbar slot",
    "look            describe the block you are looking at",
    "drop [item]     throw the selected item (or that one) in front of you (also Q)",
    "icons           export every item icon as PNG (to retouch and reuse)",
    "mode [name]     show or change the game mode (survival, creative, spectator)",
    "zoom <meters>   camera distance: 0 first person, up to 32 third person, more = map",
    "set [key value] show or change a setting (fov, view, far, ...)",
    "menu            save and go back to the main menu",
    "quit            close the game",
];

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Clear,
    Position,
    Stats,
    /// Bloques cuyo identificador o nombre contiene el texto (todos si no hay).
    Blocks(Option<String>),
    Items(Option<String>),
    /// Dar un ítem: a la primera ranura libre o a la indicada (0-8).
    Give(String, Option<usize>),
    /// Poner en la ranura activa el ítem con este identificador.
    Block(String),
    /// Describir el bloque apuntado.
    Look,
    /// Exportar los iconos de los ítems a PNG.
    Icons,
    /// Tirar el ítem activo o el indicado.
    Drop(Option<String>),
    /// Sin argumento dice el modo de juego; con nombre lo cambia.
    Mode(Option<String>),
    /// Distancia de cámara en metros.
    Zoom(f32),
    /// Sin argumentos lista los ajustes; con clave y valor cambia uno.
    Set(Option<(String, String)>),
    Menu,
    Quit,
    /// No es del cliente: la línea se envía al servidor.
    Server(String),
}

/// Filtro opcional de `blocks` e `items` (puede llevar espacios).
fn filter(args: &[&str]) -> Option<String> {
    (!args.is_empty()).then(|| args.join(" "))
}

/// Interpreta una línea de consola. El error es el mensaje para quien la escribió.
pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let mut words = line.split_whitespace();
    let name = words.next().ok_or("Type a command, or 'help' for a list.")?;
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
        "clear" => no_args(Command::Clear),
        "pos" => no_args(Command::Position),
        "stats" => no_args(Command::Stats),
        "blocks" => Ok(Command::Blocks(filter(&args))),
        "items" => Ok(Command::Items(filter(&args))),
        "look" => no_args(Command::Look),
        "icons" => no_args(Command::Icons),
        "drop" => match args[..] {
            [] => Ok(Command::Drop(None)),
            [item] => Ok(Command::Drop(Some(item.to_string()))),
            _ => Err("Usage: drop [item]".into()),
        },
        "give" => match args[..] {
            [item] => Ok(Command::Give(item.to_string(), None)),
            [item, slot] => match slot.parse::<usize>() {
                Ok(slot @ 1..=9) => Ok(Command::Give(item.to_string(), Some(slot - 1))),
                _ => Err(format!("'{slot}' is not a hotbar slot (1-9).")),
            },
            _ => Err("Usage: give <item> [slot]".into()),
        },
        "menu" => no_args(Command::Menu),
        "quit" | "exit" => no_args(Command::Quit),
        "block" => match args[..] {
            [id] => Ok(Command::Block(id.to_string())),
            _ => Err("Usage: block <item>".into()),
        },
        "mode" => match args[..] {
            [] => Ok(Command::Mode(None)),
            [mode] => Ok(Command::Mode(Some(mode.to_string()))),
            _ => Err("Usage: mode [survival|creative|spectator]".into()),
        },
        "zoom" => match args[..] {
            [meters] => match meters.parse::<f32>() {
                Ok(meters) if meters.is_finite() => Ok(Command::Zoom(meters)),
                _ => Err(format!("'{meters}' is not a distance in meters.")),
            },
            _ => Err("Usage: zoom <meters>".into()),
        },
        "set" => match args[..] {
            [] => Ok(Command::Set(None)),
            // El valor puede llevar espacios (un nombre de jugador).
            [key, ..] if args.len() >= 2 => {
                Ok(Command::Set(Some((key.to_string(), args[1..].join(" ")))))
            }
            _ => Err("Usage: set <key> <value>".into()),
        },
        _ => Ok(Command::Server(line.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_commands_are_recognized() {
        assert_eq!(parse(" help "), Ok(Command::Help));
        assert_eq!(parse("exit"), Ok(Command::Quit));
        assert_eq!(parse("block base:stone"), Ok(Command::Block("base:stone".into())));
        assert_eq!(parse("blocks"), Ok(Command::Blocks(None)));
        assert_eq!(parse("items piedra roja"), Ok(Command::Items(Some("piedra roja".into()))));
        assert_eq!(parse("give base:lamp"), Ok(Command::Give("base:lamp".into(), None)));
        assert_eq!(parse("give lamp 9"), Ok(Command::Give("lamp".into(), Some(8))));
        assert_eq!(parse("look"), Ok(Command::Look));
        assert_eq!(parse("drop"), Ok(Command::Drop(None)));
        assert_eq!(parse("drop base:door"), Ok(Command::Drop(Some("base:door".into()))));
        assert_eq!(parse("mode"), Ok(Command::Mode(None)));
        assert_eq!(parse("mode creative"), Ok(Command::Mode(Some("creative".into()))));
        assert_eq!(parse("zoom 12.5"), Ok(Command::Zoom(12.5)));
        assert_eq!(parse("set"), Ok(Command::Set(None)));
        assert_eq!(parse("set fov 90"), Ok(Command::Set(Some(("fov".into(), "90".into())))));
        assert_eq!(
            parse("set name Dante Storm"),
            Ok(Command::Set(Some(("name".into(), "Dante Storm".into()))))
        );
    }

    #[test]
    fn bad_usage_is_explained() {
        assert!(parse("   ").is_err());
        assert!(parse("clear now").is_err());
        assert_eq!(parse("block"), Err("Usage: block <item>".into()));
        assert!(parse("give lamp 0").is_err());
        assert!(parse("give").is_err());
        assert!(parse("zoom far").is_err());
        assert!(parse("zoom").is_err());
        assert_eq!(parse("set fov"), Err("Usage: set <key> <value>".into()));
    }

    #[test]
    fn everything_else_goes_to_the_server() {
        assert_eq!(parse("tp 0 80 0"), Ok(Command::Server("tp 0 80 0".into())));
        assert_eq!(parse("  seed"), Ok(Command::Server("seed".into())));
    }
}
