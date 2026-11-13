//! Mundos guardados en disco: listarlos, validar nombres y borrarlos.
//!
//! No sabe nada de interfaces; lo usan las pantallas y se prueba por separado.

use std::path::Path;

pub const DEFAULT_WORLD: &str = "default";
const MAX_WORLD_NAME: usize = 32;

/// Un nombre de mundo es también el de su carpeta: solo caracteres seguros.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Type a name for the new world.");
    }
    if name.chars().count() > MAX_WORLD_NAME {
        return Err("That name is too long (32 characters at most).");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Use only letters, digits, '-' and '_'.");
    }
    Ok(())
}

/// Lo que se sabe de un mundo guardado sin abrirlo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldInfo {
    /// Nombre del mundo, que es también el de su carpeta.
    pub name: String,
    /// Semilla, si `world.toml` se puede leer.
    pub seed: Option<u32>,
}

fn read_seed(world_toml: &Path) -> Option<u32> {
    let table: toml::Table = std::fs::read_to_string(world_toml).ok()?.parse().ok()?;
    u32::try_from(table.get("seed")?.as_integer()?).ok()
}

/// Mundos existentes: carpetas de `dir` con un `world.toml`, por orden alfabético.
pub fn list(dir: &Path) -> Vec<WorldInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut worlds: Vec<WorldInfo> = entries
        .flatten()
        .filter(|entry| entry.path().join("world.toml").is_file())
        .filter_map(|entry| {
            let seed = read_seed(&entry.path().join("world.toml"));
            Some(WorldInfo { name: entry.file_name().into_string().ok()?, seed })
        })
        .collect();
    worlds.sort_by_key(|world| world.name.to_lowercase());
    worlds
}

/// Semilla escrita por el jugador: un número tal cual o, si es otro texto, un número
/// derivado de él (el mismo texto da siempre el mismo mundo). `None` si no escribió nada.
pub fn parse_seed(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.parse().unwrap_or_else(|_| {
        // FNV-1a: estable entre versiones y plataformas, a diferencia de `Hash`.
        text.bytes().fold(0x811C_9DC5u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(0x0100_0193))
    }))
}

/// Semilla para un mundo nuevo cuando el jugador no elige ninguna.
pub fn random_seed() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs() as u32).wrapping_mul(0x9E37_79B9) ^ now.subsec_nanos() ^ std::process::id()
}

/// Borra la carpeta de un mundo. Solo acepta nombres con forma válida y carpetas que
/// sean de verdad un mundo, para que nunca pueda borrarse nada fuera de `dir`.
pub fn delete(dir: &Path, world: &str) -> std::io::Result<()> {
    let invalid =
        |reason: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason.to_string());
    validate_name(world).map_err(invalid)?;
    let path = dir.join(world);
    if !path.join("world.toml").is_file() {
        return Err(invalid("not a world folder"));
    }
    log::info!("deleting world {}", path.display());
    std::fs::remove_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_numbers_or_derived_from_text() {
        assert_eq!(parse_seed("  "), None);
        assert_eq!(parse_seed(" 1337 "), Some(1337));
        let named = parse_seed("tokikura").unwrap();
        assert_eq!(parse_seed("tokikura"), Some(named), "the same text gives the same world");
        assert_ne!(parse_seed("Tokikura"), Some(named));
        // Un número que no cabe en 32 bits se trata como texto, no se rechaza.
        assert!(parse_seed("99999999999").is_some());
    }

    #[test]
    fn listing_reads_names_and_seeds() {
        let dir = std::env::temp_dir().join(format!("tokikura-worlds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, toml) in [("beta", "seed = 42\nblocks = []\n"), ("Alpha", "not toml [")] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("world.toml"), toml).unwrap();
        }
        std::fs::create_dir_all(dir.join("not-a-world")).unwrap();
        assert_eq!(
            list(&dir),
            [
                WorldInfo { name: "Alpha".into(), seed: None },
                WorldInfo { name: "beta".into(), seed: Some(42) },
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
