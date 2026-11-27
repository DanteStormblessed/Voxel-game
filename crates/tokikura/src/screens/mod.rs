//! Pantallas de Tokikura. Cada una es un archivo: su estado, sus mensajes y su vista.
//!
//! Una pantalla no habla con el motor. Cuando quiere que pase algo fuera de ella (abrir
//! un mundo, cambiar de pantalla, cerrar el juego) lo deja anotado como una petición
//! (`Request`) que `game.rs` recoge. Lo que necesita saber del juego (la barra, el
//! contenido) se lo copia `game.rs` antes de dibujarla.
//!
//! Todas son provisionales: lo que importa es su lógica (mensajes y peticiones); el
//! aspecto vive en `view` y en `style.rs` para poder sustituirlo sin tocar lo demás.

pub mod biome_editor;
pub mod console;
pub mod hud;
pub mod inventory;
pub mod main_menu;
pub mod node_graph;
pub mod pause_menu;
pub mod prefabs;
pub mod settings;
pub mod sky;
pub mod style;
pub mod world_create;
pub mod world_select;
