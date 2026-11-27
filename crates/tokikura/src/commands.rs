//! Comandos de la consola de desarrollador que resuelve el propio cliente.
//!
//! Aquí solo se interpreta el texto; `game.rs` los ejecuta. Lo que no es un comando del
//! cliente se envía tal cual al servidor (`tp`, `seed`...), que tiene los suyos.

use crate::builder::RegionAction;

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
    "set [key value] show or change a setting (fov, view, far, shadows, ...)",
    "sky             open the sky and light panel (also F6)",
    "env [key value] show or change the sky, sun and shadow values (env reset, env save)",
    "time ...        time of day: see the server commands",
    "pos1 / pos2 [x y z]  mark a corner of the selection (at your feet, or there)",
    "desel           clear the selection",
    "copy / cut / erase   copy, cut or empty the selection",
    "paste [here]    start placing the clipboard (here: place it right now)",
    "rotate          turn what you are placing a quarter turn",
    "pasteair [on|off]  whether empty space in the clipboard also clears blocks",
    "undo            undo the last building change",
    "prefab save|load|delete <name>   save the selection, or load / delete a prefab",
    "prefab list     list the saved prefabs",
    "prefabs         open the prefab menu (also P)",
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
    /// Abrir el panel de cielo y luz.
    Sky,
    /// Sin argumentos lista los valores del ambiente; con clave y valor cambia uno.
    Env(Option<(String, String)>),
    /// Volver al ambiente por defecto.
    EnvReset,
    /// Guardar el ambiente en su archivo.
    EnvSave,
    /// Marcar una esquina de la selección (la primera o la segunda): en ese voxel o, sin
    /// coordenadas, a los pies del jugador.
    Corner(bool, Option<[i32; 3]>),
    Deselect,
    /// Copiar, cortar o vaciar la selección.
    Region(RegionAction),
    /// Empezar a colocar el portapapeles o, con `here`, pegarlo ya.
    Paste { here: bool },
    Rotate,
    /// Sin argumento dice si se pega también el aire; con él lo cambia.
    PasteAir(Option<bool>),
    Undo,
    PrefabSave(String),
    PrefabLoad(String),
    PrefabDelete(String),
    PrefabList,
    /// Abrir el menú de prefabs.
    Prefabs,
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
        "pos1" | "pos2" => {
            let first = name == "pos1";
            match args[..] {
                [] => Ok(Command::Corner(first, None)),
                [x, y, z] => match (x.parse(), y.parse(), z.parse()) {
                    (Ok(x), Ok(y), Ok(z)) => Ok(Command::Corner(first, Some([x, y, z]))),
                    _ => Err(format!("Usage: {name} [x y z] (whole numbers)")),
                },
                _ => Err(format!("Usage: {name} [x y z]")),
            }
        }
        "desel" => no_args(Command::Deselect),
        "copy" => no_args(Command::Region(RegionAction::Copy)),
        "cut" => no_args(Command::Region(RegionAction::Cut)),
        "erase" => no_args(Command::Region(RegionAction::Erase)),
        "undo" => no_args(Command::Undo),
        "rotate" => no_args(Command::Rotate),
        "paste" => match args[..] {
            [] => Ok(Command::Paste { here: false }),
            ["here"] => Ok(Command::Paste { here: true }),
            _ => Err("Usage: paste [here]".into()),
        },
        "pasteair" => match args[..] {
            [] => Ok(Command::PasteAir(None)),
            ["on"] => Ok(Command::PasteAir(Some(true))),
            ["off"] => Ok(Command::PasteAir(Some(false))),
            _ => Err("Usage: pasteair [on|off]".into()),
        },
        "prefab" => match args[..] {
            ["save", prefab] => Ok(Command::PrefabSave(prefab.to_string())),
            ["load", prefab] => Ok(Command::PrefabLoad(prefab.to_string())),
            ["delete", prefab] => Ok(Command::PrefabDelete(prefab.to_string())),
            ["list"] => Ok(Command::PrefabList),
            _ => Err("Usage: prefab save|load|delete <name>, or prefab list".into()),
        },
        "prefabs" => no_args(Command::Prefabs),
        "sky" => no_args(Command::Sky),
        "env" => match args[..] {
            [] => Ok(Command::Env(None)),
            ["reset"] => Ok(Command::EnvReset),
            ["save"] => Ok(Command::EnvSave),
            [key, value] => Ok(Command::Env(Some((key.to_string(), value.to_string())))),
            _ => Err("Usage: env [key value], env reset or env save".into()),
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
        assert_eq!(parse("pos1"), Ok(Command::Corner(true, None)));
        assert_eq!(parse("pos2 4 -5 6"), Ok(Command::Corner(false, Some([4, -5, 6]))));
        assert_eq!(parse("cut"), Ok(Command::Region(RegionAction::Cut)));
        assert_eq!(parse("paste"), Ok(Command::Paste { here: false }));
        assert_eq!(parse("paste here"), Ok(Command::Paste { here: true }));
        assert_eq!(parse("pasteair on"), Ok(Command::PasteAir(Some(true))));
        assert_eq!(parse("prefab save trees/oak"), Ok(Command::PrefabSave("trees/oak".into())));
        assert_eq!(parse("prefab list"), Ok(Command::PrefabList));
        assert_eq!(parse("prefabs"), Ok(Command::Prefabs));
        assert_eq!(parse("set"), Ok(Command::Set(None)));
        assert_eq!(parse("sky"), Ok(Command::Sky));
        assert_eq!(parse("env"), Ok(Command::Env(None)));
        assert_eq!(parse("env reset"), Ok(Command::EnvReset));
        assert_eq!(
            parse("env day.zenith #3366CC"),
            Ok(Command::Env(Some(("day.zenith".into(), "#3366CC".into()))))
        );
        assert!(parse("env sun").is_err());
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
        assert!(parse("pos1 1 2").is_err());
        assert!(parse("pos2 a b c").is_err());
        assert!(parse("paste there").is_err());
        assert!(parse("prefab save").is_err());
        assert!(parse("prefab").is_err());
        assert!(parse("zoom").is_err());
        assert_eq!(parse("set fov"), Err("Usage: set <key> <value>".into()));
    }

    #[test]
    fn everything_else_goes_to_the_server() {
        assert_eq!(parse("tp 0 80 0"), Ok(Command::Server("tp 0 80 0".into())));
        assert_eq!(parse("  seed"), Ok(Command::Server("seed".into())));
        assert_eq!(parse("time set dusk"), Ok(Command::Server("time set dusk".into())));
    }
}
