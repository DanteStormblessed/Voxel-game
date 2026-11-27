//! Herramienta de construcción: selección de una caja, portapapeles y pegado de prefabs.
//!
//! Aquí vive solo el estado del jugador (qué esquinas ha marcado, qué tiene copiado, si
//! está colocando algo y cómo girado). Copiar, guardar y pegar los hace el servidor: este
//! módulo decide qué orden (`PrefabOp`) enviarle y qué marcas dibujar. No toca el motor,
//! así que se prueba con tests normales; `game.rs` lo conecta con teclas, clics y menú.

use std::sync::Arc;

use glam::{DVec3, IVec3, Vec3};
use kuravoxel_client::session::{Ghost, Outline, Overlay, ToolButton};
use kuravoxel_common::content::ContentData;
use kuravoxel_common::prefab::{Prefab, PrefabInfo, MAX_PREFAB_SIDE};
use kuravoxel_common::protocol::PrefabOp;

/// Valor de `tool` (en `items/*.toml`) del ítem que activa la selección.
pub const SELECTION_TOOL: &str = "selection";
/// Alcance del rayo con la herramienta activa, en metros.
pub const TOOL_REACH: f64 = 96.0;

const SELECTION_COLOR: [f32; 3] = [1.0, 0.82, 0.12];
const FIRST_COLOR: [f32; 3] = [1.0, 0.16, 0.12];
const SECOND_COLOR: [f32; 3] = [0.12, 0.42, 1.0];
const PASTE_COLOR: [f32; 3] = [0.2, 1.0, 0.35];
const FACE_COLOR: [f32; 3] = [1.0, 1.0, 1.0];
const HOT_COLOR: [f32; 3] = [0.25, 1.0, 0.45];

/// Una cara de la caja seleccionada: en qué eje (0 = X, 1 = Y, 2 = Z) y si es la del
/// lado positivo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub axis: usize,
    pub positive: bool,
}

/// Arrastre de una cara en curso.
struct Drag {
    face: Face,
    /// Punto de la cara donde se agarró.
    grab: DVec3,
    /// Coordenada, en el eje de la cara, que tenía la esquina que se mueve.
    start: i32,
}

/// En qué punto está el dibujo de una selección nueva.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum Stage {
    /// No se está dibujando: no hay selección, o ya está completa.
    #[default]
    Idle,
    /// Primer clic dado y botón aún pulsado: la base se estira hasta donde se apunta.
    Base,
    /// Primer clic suelto sin arrastrar: el siguiente clic cierra la base.
    WaitB,
    /// Base cerrada: la altura sigue a la mira hasta el clic que la confirma. `low` y
    /// `high` son las alturas de la base y `grab` el punto desde el que se mide.
    Height { low: i32, high: i32, grab: DVec3 },
}

const ALL_FACES: [Face; 6] = [
    Face { axis: 0, positive: false },
    Face { axis: 0, positive: true },
    Face { axis: 1, positive: false },
    Face { axis: 1, positive: true },
    Face { axis: 2, positive: false },
    Face { axis: 2, positive: true },
];

/// Manilla de una cara de la caja `min..max`: un cuadrado en su centro, del 40 % del lado
/// corto de la cara (entre 0,6 y 3 m). Devuelve su centro y la mitad de su lado.
fn handle(min: DVec3, max: DVec3, face: Face) -> (DVec3, f64) {
    let mut center = (min + max) * 0.5;
    center[face.axis] = if face.positive { max[face.axis] } else { min[face.axis] };
    let (a, b) = ((face.axis + 1) % 3, (face.axis + 2) % 3);
    let side = (0.4 * (max[a] - min[a]).min(max[b] - min[b])).clamp(0.6, 3.0);
    (center, side * 0.5)
}

/// Distancia a la que un rayo cruza la manilla de una cara, si la cruza.
fn ray_handle(origin: DVec3, dir: DVec3, min: DVec3, max: DVec3, face: Face) -> Option<f64> {
    let (center, half) = handle(min, max, face);
    let axis = face.axis;
    if dir[axis].abs() < 1e-9 {
        return None;
    }
    let distance = (center[axis] - origin[axis]) / dir[axis];
    let point = origin + dir * distance;
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    let inside = (point[a] - center[a]).abs() <= half && (point[b] - center[b]).abs() <= half;
    (distance > 0.0 && inside).then_some(distance)
}

/// Qué hacer con la caja seleccionada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionAction {
    Copy,
    Cut,
    /// Vaciarla.
    Erase,
}

#[derive(Default)]
pub struct Builder {
    first: Option<IVec3>,
    second: Option<IVec3>,
    /// Lo último copiado o cargado, tal como lo tiene el servidor, y su nombre si viene
    /// de un prefab guardado.
    clipboard: Option<Arc<Prefab>>,
    clipboard_name: String,
    /// El portapapeles girado `turns` cuartos de vuelta: lo que se enseña al colocar.
    shown: Option<Arc<Prefab>>,
    turns: u8,
    /// Se está colocando el portapapeles: la vista previa sigue a la mira.
    pasting: bool,
    /// Los huecos del portapapeles también vacían lo que haya.
    pub paste_air: bool,
    /// Bloques que sube (o baja) lo que se coloca respecto a donde se apunta.
    lift: i32,
    /// Prefabs guardados, según la última lista del servidor.
    prefabs: Vec<PrefabInfo>,
    /// Último resultado del servidor y si fue un éxito.
    status: Option<(String, bool)>,
    /// Cara de la selección a la que apunta la mira y el punto exacto.
    hover: Option<(Face, DVec3)>,
    drag: Option<Drag>,
    stage: Stage,
}

fn size_text(size: IVec3) -> String {
    format!("{}x{}x{}", size.x, size.y, size.z)
}

impl Builder {
    // --- Selección ---

    /// Marca una esquina de la selección.
    pub fn set_corner(&mut self, first: bool, pos: IVec3) {
        self.stage = Stage::Idle;
        if first {
            self.first = Some(pos);
        } else {
            self.second = Some(pos);
        }
    }

    pub fn deselect(&mut self) {
        self.first = None;
        self.second = None;
        self.stage = Stage::Idle;
        self.clear_aim();
    }

    /// La base está cerrada: se pasa a elegir la altura, midiendo desde lo alto de la
    /// segunda esquina.
    fn enter_height(&mut self) {
        let (Some(first), Some(second)) = (self.first.as_mut(), self.second.as_mut()) else { return };
        let (low, high) = (first.y.min(second.y), first.y.max(second.y));
        first.y = low;
        second.y = high;
        let grab = DVec3::new(second.x as f64 + 0.5, high as f64 + 1.0, second.z as f64 + 0.5);
        self.stage = Stage::Height { low, high, grab };
    }

    /// La altura sigue a la mira: hacia arriba crece el techo, hacia abajo el suelo.
    fn height_to(&mut self, origin: DVec3, dir: DVec3, low: i32, high: i32, grab: DVec3) {
        let spread = 1.0 - dir.y * dir.y;
        if spread < 1e-3 {
            return;
        }
        let to_grab = grab - origin;
        let moved = (dir.y * dir.dot(to_grab) - to_grab.y) / spread;
        let limit = MAX_PREFAB_SIDE - 1 - (high - low);
        let moved = (moved.clamp(-2.0 * TOOL_REACH, 2.0 * TOOL_REACH).round() as i32).clamp(-limit, limit);
        if let (Some(first), Some(second)) = (self.first.as_mut(), self.second.as_mut()) {
            first.y = low + moved.min(0);
            second.y = high + moved.max(0);
        }
    }

    /// Se está dibujando una selección nueva (base o altura).
    #[cfg(test)]
    pub fn drawing(&self) -> bool {
        self.stage != Stage::Idle
    }

    // --- Caras de la selección: apuntar, arrastrar y empujar ---

    /// Cada frame con la vara en la mano: el rayo de la mira y el bloque al que apunta.
    /// Dibujando una selección, la base o la altura siguen a la mira. Si se arrastra una
    /// cara, la mueve; si no, mira a qué **manilla** se apunta (el cuadrado del centro de
    /// cada cara): la primera que cruza el rayo o, con `far`, la última. Ni los bloques
    /// ni la propia caja las tapan: la de la cara de abajo se agarra mirándola desde
    /// arriba, a través de la caja.
    pub fn aim(&mut self, origin: [f64; 3], dir: [f32; 3], target: Option<[i32; 3]>, far: bool) {
        let origin = DVec3::from_array(origin);
        let dir = Vec3::from_array(dir).as_dvec3().normalize_or_zero();
        match self.stage {
            Stage::Base | Stage::WaitB => {
                self.hover = None;
                if let Some(target) = target {
                    self.second = Some(IVec3::from_array(target));
                }
                return;
            }
            Stage::Height { low, high, grab } => {
                self.hover = None;
                self.height_to(origin, dir, low, high, grab);
                return;
            }
            Stage::Idle => {}
        }
        if self.drag.is_some() {
            self.drag_to(origin, dir);
            return;
        }
        self.hover = None;
        let Some((min, max)) = self.selection().filter(|_| !self.pasting && dir != DVec3::ZERO) else {
            return;
        };
        let (min, max) = (min.as_dvec3(), (max + IVec3::ONE).as_dvec3());
        let hits = ALL_FACES.into_iter().filter_map(|face| {
            let distance = ray_handle(origin, dir, min, max, face)?;
            (distance <= 2.0 * TOOL_REACH).then_some((distance, face))
        });
        let hit = if far { hits.max_by(|a, b| a.0.total_cmp(&b.0)) } else { hits.min_by(|a, b| a.0.total_cmp(&b.0)) };
        if let Some((distance, face)) = hit {
            self.hover = Some((face, origin + dir * distance));
        }
    }

    /// Olvida a qué se apuntaba y suelta lo que se arrastraba (la vara ya no está en uso).
    pub fn clear_aim(&mut self) {
        self.hover = None;
        self.drag = None;
        // Una base a medio estirar se queda esperando su segundo clic.
        if self.stage == Stage::Base {
            self.stage = Stage::WaitB;
        }
    }

    /// Cara a la que apunta la mira (o la que se está arrastrando).
    pub fn hover_face(&self) -> Option<Face> {
        self.drag.as_ref().map(|drag| drag.face).or(self.hover.map(|(face, _)| face))
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Se ha soltado el botón: termina el arrastre de una cara o el de la base. Una base
    /// que no se ha estirado espera un segundo clic; si se estiró, toca la altura.
    pub fn release(&mut self) {
        self.drag = None;
        if self.stage == Stage::Base {
            if self.first == self.second {
                self.stage = Stage::WaitB;
            } else {
                self.enter_height();
            }
        }
    }

    /// De las dos esquinas, ¿es la primera la que define esa cara?
    fn first_owns(&self, face: Face) -> Option<bool> {
        let (first, second) = (self.first?[face.axis], self.second?[face.axis]);
        Some(if face.positive { first > second } else { first < second })
    }

    /// Coordenada actual de la esquina que define una cara.
    fn face_value(&self, face: Face) -> Option<i32> {
        let first = self.first_owns(face)?;
        Some(if first { self.first?[face.axis] } else { self.second?[face.axis] })
    }

    /// Lleva una cara a `value` sin que cruce la de enfrente ni pase del tamaño máximo.
    fn set_face(&mut self, face: Face, value: i32) {
        let Some(first_owns) = self.first_owns(face) else { return };
        let (Some(first), Some(second)) = (self.first.as_mut(), self.second.as_mut()) else { return };
        let (moving, other) = if first_owns { (first, *second) } else { (second, *first) };
        let (other, limit) = (other[face.axis], MAX_PREFAB_SIDE - 1);
        moving[face.axis] =
            if face.positive { value.clamp(other, other + limit) } else { value.clamp(other - limit, other) };
    }

    /// Empuja hacia fuera (`steps` > 0) o hacia dentro la cara a la que se apunta.
    /// Devuelve `false` si no se apunta a ninguna.
    pub fn nudge(&mut self, steps: i32) -> bool {
        let (Some(face), false) = (self.hover_face(), self.dragging()) else { return false };
        let Some(value) = self.face_value(face) else { return false };
        self.set_face(face, value + if face.positive { steps } else { -steps });
        true
    }

    /// Arrastre: la cara sigue a la mira. Se busca el punto de la recta que pasa por
    /// donde se agarró, perpendicular a la cara, que queda más cerca del rayo de la mira.
    fn drag_to(&mut self, origin: DVec3, dir: DVec3) {
        let Some(drag) = &self.drag else { return };
        let (face, start) = (drag.face, drag.start);
        let normal = DVec3::AXES[face.axis];
        let to_grab = drag.grab - origin;
        let along = normal.dot(dir);
        let spread = 1.0 - along * along;
        // Mirando casi a lo largo del eje no hay forma de saber cuánto se quiere mover.
        if spread < 1e-3 {
            return;
        }
        let moved = (along * dir.dot(to_grab) - normal.dot(to_grab)) / spread;
        let moved = moved.clamp(-2.0 * TOOL_REACH, 2.0 * TOOL_REACH).round() as i32;
        self.set_face(face, start + moved);
    }

    #[cfg(test)]
    pub fn corners(&self) -> (Option<IVec3>, Option<IVec3>) {
        (self.first, self.second)
    }

    /// Esquinas mínima y máxima de la caja, si están marcadas las dos.
    pub fn selection(&self) -> Option<(IVec3, IVec3)> {
        let (a, b) = (self.first?, self.second?);
        Some((a.min(b), a.max(b)))
    }

    fn require_selection(&self) -> Result<(IVec3, IVec3), String> {
        self.selection()
            .ok_or_else(|| "Mark both corners first: left and right click with the selection wand.".to_string())
    }

    /// Orden para copiar, cortar o vaciar la selección.
    pub fn region_op(&self, action: RegionAction) -> Result<PrefabOp, String> {
        let (min, max) = self.require_selection()?;
        let (min, max) = (min.to_array(), max.to_array());
        Ok(match action {
            RegionAction::Copy => PrefabOp::Copy { min, max },
            RegionAction::Cut => PrefabOp::Cut { min, max },
            RegionAction::Erase => PrefabOp::Fill { min, max, block: 0 },
        })
    }

    /// Orden para guardar la selección como prefab.
    pub fn save_op(&self, name: &str) -> Result<PrefabOp, String> {
        let (min, max) = self.require_selection()?;
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the prefab a name, like trees/oak.".into());
        }
        Ok(PrefabOp::Save { name: name.to_string(), min: min.to_array(), max: max.to_array() })
    }

    // --- Portapapeles y pegado ---

    /// El servidor ha dejado esto en el portapapeles.
    pub fn set_clipboard(&mut self, prefab: Arc<Prefab>, name: String) {
        self.shown = Some(prefab.clone());
        self.clipboard = Some(prefab);
        self.clipboard_name = name;
        self.turns = 0;
        self.lift = 0;
    }

    pub fn has_clipboard(&self) -> bool {
        self.clipboard.is_some()
    }

    pub fn pasting(&self) -> bool {
        self.pasting
    }

    /// Empieza a colocar el portapapeles (aunque aún esté de camino desde el servidor).
    pub fn start_paste(&mut self) {
        self.pasting = true;
    }

    pub fn stop_paste(&mut self) {
        self.pasting = false;
    }

    /// Gira un cuarto de vuelta lo que se va a colocar.
    pub fn rotate(&mut self, content: &ContentData) {
        self.turns = (self.turns + 1) % 4;
        self.refresh(content);
    }

    /// Recalcula la vista previa (al girar o al cambiar el contenido).
    pub fn refresh(&mut self, content: &ContentData) {
        self.shown = self.clipboard.as_ref().map(|prefab| match self.turns {
            0 => prefab.clone(),
            turns => Arc::new(prefab.rotated(turns, content)),
        });
    }

    /// Sube o baja lo que se coloca.
    pub fn lift(&mut self, blocks: i32) {
        self.lift = (self.lift + blocks).clamp(-64, 64);
    }

    /// Dónde cae el anclaje si se apunta al hueco `before`.
    fn paste_target(&self, before: [i32; 3]) -> IVec3 {
        IVec3::from_array(before) + IVec3::new(0, self.lift, 0)
    }

    /// Orden para colocar el portapapeles apuntando al hueco `before`.
    pub fn paste_op(&self, before: Option<[i32; 3]>) -> Result<PrefabOp, String> {
        if self.clipboard.is_none() {
            return Err("The clipboard is empty: copy a selection or load a prefab first.".into());
        }
        Ok(self.paste_at(before.ok_or("Aim at a block to place it.")?))
    }

    /// Como `paste_op`, sin comprobar el portapapeles: el del servidor es el que cuenta
    /// (una orden de consola puede ir justo detrás de la que lo llena).
    pub fn paste_at(&self, before: [i32; 3]) -> PrefabOp {
        PrefabOp::Paste { pos: self.paste_target(before).to_array(), turns: self.turns, air: self.paste_air }
    }

    /// Clic con la herramienta activa.
    ///
    /// Colocando un prefab: el derecho pega (y se puede seguir pegando) y el izquierdo lo
    /// deja; devuelve la orden para el servidor. Con la vara, todo va con el izquierdo:
    /// el primero marca una esquina de la base (manteniéndolo, la base se estira hasta
    /// soltar), el segundo la esquina opuesta y el tercero confirma la altura. Con la
    /// selección hecha, sobre la manilla de una cara la agarra para arrastrarla y en
    /// cualquier otro sitio empieza otra selección. El derecho y el central la borran.
    pub fn click(&mut self, button: ToolButton, hit: Option<[i32; 3]>, before: Option<[i32; 3]>) -> Result<Option<PrefabOp>, String> {
        if self.pasting {
            return match button {
                ToolButton::Secondary => self.paste_op(before).map(Some),
                _ => {
                    self.stop_paste();
                    Ok(None)
                }
            };
        }
        if button != ToolButton::Primary {
            self.deselect();
            return Ok(None);
        }
        let hit = hit.map(IVec3::from_array);
        match self.stage {
            Stage::Height { .. } => self.stage = Stage::Idle,
            Stage::Base | Stage::WaitB => {
                self.second = Some(hit.or(self.second).ok_or("Nothing in reach: aim at a block.")?);
                self.enter_height();
            }
            Stage::Idle => {
                if let Some((face, grab)) = self.hover {
                    if let Some(start) = self.face_value(face) {
                        self.drag = Some(Drag { face, grab, start });
                    }
                    return Ok(None);
                }
                let hit = hit.ok_or("Nothing in reach: aim at a block.")?;
                self.first = Some(hit);
                self.second = Some(hit);
                self.stage = Stage::Base;
            }
        }
        Ok(None)
    }

    /// Medidas de la selección para enseñarlas sobre ella: punto del mundo y texto. Van
    /// en el centro de las tres aristas que salen de la esquina de arriba más cercana a
    /// la cámara: ancho (X), largo (Z) y alto.
    pub fn labels(&self, camera: [f64; 3]) -> Vec<([f64; 3], String)> {
        let Some((min, max)) = self.selection() else { return Vec::new() };
        let size = max - min + IVec3::ONE;
        let (lo, hi) = (min.as_dvec3(), (max + IVec3::ONE).as_dvec3());
        let mid = (lo + hi) * 0.5;
        let near = |axis: usize| if (camera[axis] - lo[axis]).abs() < (camera[axis] - hi[axis]).abs() { lo[axis] } else { hi[axis] };
        let (x, z) = (near(0), near(2));
        vec![
            ([mid.x, hi.y, z], size.x.to_string()),
            ([x, hi.y, mid.z], size.z.to_string()),
            ([x, mid.y, z], size.y.to_string()),
        ]
    }

    // --- Lo que se enseña ---

    pub fn set_prefabs(&mut self, prefabs: Vec<PrefabInfo>) {
        self.prefabs = prefabs;
    }

    pub fn prefabs(&self) -> &[PrefabInfo] {
        &self.prefabs
    }

    pub fn set_status(&mut self, text: String, ok: bool) {
        self.status = Some((text, ok));
    }

    pub fn status(&self) -> Option<(&str, bool)> {
        self.status.as_ref().map(|(text, ok)| (text.as_str(), *ok))
    }

    /// Descripción de la selección: esquinas y tamaño.
    pub fn selection_text(&self) -> String {
        let corner = |pos: Option<IVec3>| pos.map_or("-".to_string(), |p| format!("{} {} {}", p.x, p.y, p.z));
        let size = self.selection().map_or(String::new(), |(min, max)| {
            let size = max - min + IVec3::ONE;
            format!("  |  {} ({} blocks)", size_text(size), size.x * size.y * size.z)
        });
        format!("Corner 1: {}  |  Corner 2: {}{size}", corner(self.first), corner(self.second))
    }

    /// Descripción del portapapeles.
    pub fn clipboard_text(&self) -> String {
        let Some(prefab) = &self.shown else { return "Clipboard: empty".into() };
        let name = if self.clipboard_name.is_empty() { "copied selection" } else { &self.clipboard_name };
        let turn = if self.turns == 0 { String::new() } else { format!(", turned {}°", self.turns as u32 * 90) };
        format!("Clipboard: {name} ({}{turn})", size_text(prefab.size()))
    }

    /// Líneas de ayuda para el HUD mientras la herramienta está activa.
    pub fn hud_lines(&self) -> Vec<String> {
        if self.pasting {
            let lift = if self.lift == 0 { String::new() } else { format!("  |  raised {:+}", self.lift) };
            let air = if self.paste_air { "  |  replacing with air" } else { "" };
            return vec![
                format!("Placing  |  {}{lift}{air}", self.clipboard_text()),
                "RMB place   LMB / Esc stop   R rotate   PgUp / PgDn raise / lower   Ctrl+Z undo".into(),
            ];
        }
        let help = match self.stage {
            Stage::Base => "Release to close the base (or click the opposite corner)",
            Stage::WaitB => "LMB: opposite corner of the base",
            Stage::Height { .. } => "Aim up or down to set the height   LMB: confirm   RMB: cancel",
            Stage::Idle if self.selection().is_some() => {
                "Aim at the square on a side: hold LMB to drag it, or push / pull with the wheel   LMB elsewhere: new box   RMB: clear"
            }
            Stage::Idle => "LMB: first corner (hold and drag to draw the base), then the height",
        };
        vec![
            format!("Selection wand  |  {}", self.selection_text()),
            self.clipboard_text(),
            help.into(),
            "Ctrl+C copy   Ctrl+X cut   Ctrl+V place   Del erase   Ctrl+Z undo   P prefabs".into(),
        ]
    }

    /// Marcas sobre el mundo: la caja seleccionada con sus dos esquinas y, colocando,
    /// la vista previa de lo que caería apuntando al hueco `before`.
    pub fn overlay(&self, before: Option<[i32; 3]>, show_selection: bool) -> Overlay {
        let mut overlay = Overlay::default();
        // Caja de voxeles `min..=max` como contorno.
        let boxed = |min: IVec3, max: IVec3, color| Outline {
            min: min.as_dvec3().to_array(),
            max: (max + IVec3::ONE).as_dvec3().to_array(),
            color,
            weight: 1.0,
            fill: 0.0,
        };
        if show_selection {
            if let Some((min, max)) = self.selection() {
                // La caja se ve entera, también lo que queda bajo tierra: caras
                // translúcidas con rejilla de bloques.
                overlay.outlines.push(Outline { fill: 0.1, ..boxed(min, max, SELECTION_COLOR) });
                // Con la caja terminada, una manilla en el centro de cada cara; la que
                // se apunta (o se arrastra), resaltada.
                if self.stage == Stage::Idle && !self.pasting {
                    let (lo, hi) = (min.as_dvec3(), (max + IVec3::ONE).as_dvec3());
                    let active = self.hover_face();
                    for face in ALL_FACES {
                        let (center, half) = handle(lo, hi, face);
                        let mut reach = DVec3::splat(half);
                        reach[face.axis] = 0.0;
                        let hot = active == Some(face);
                        overlay.outlines.push(Outline {
                            min: (center - reach).to_array(),
                            max: (center + reach).to_array(),
                            color: if hot { HOT_COLOR } else { FACE_COLOR },
                            weight: if hot { 2.0 } else { 1.0 },
                            fill: if hot { 0.6 } else { 0.22 },
                        });
                    }
                }
            }
            overlay.outlines.extend(self.first.map(|pos| boxed(pos, pos, FIRST_COLOR)));
            overlay.outlines.extend(self.second.map(|pos| boxed(pos, pos, SECOND_COLOR)));
        }
        if let (true, Some(prefab), Some(before)) = (self.pasting, &self.shown, before) {
            let origin = self.paste_target(before) - prefab.anchor();
            let max = origin + prefab.size() - IVec3::ONE;
            overlay.outlines.push(boxed(origin, max, PASTE_COLOR));
            overlay.ghost = Some(Ghost { prefab: prefab.clone(), origin: origin.to_array() });
        }
        overlay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuravoxel_common::block::Block;
    use kuravoxel_common::content::test_content;

    fn clipboard(content: &ContentData) -> Arc<Prefab> {
        Arc::new(Prefab::capture(IVec3::new(3, 1, 1), content, |pos| Block((pos.x == 0) as u16)))
    }

    #[test]
    fn three_clicks_draw_the_base_and_the_height() {
        let mut builder = Builder::default();
        assert!(builder.region_op(RegionAction::Copy).is_err());
        assert!(builder.click(ToolButton::Primary, None, None).is_err(), "nothing in reach");

        // Clic 1 (y soltar sin arrastrar): esquina A. La base sigue a la mira hasta el clic 2.
        builder.click(ToolButton::Primary, Some([1, 10, -2]), None).unwrap();
        builder.release();
        builder.aim([0.0; 3], [0.0, 0.0, 1.0], Some([3, 10, 0]), false);
        assert_eq!(builder.selection(), Some((IVec3::new(1, 10, -2), IVec3::new(3, 10, 0))));
        builder.click(ToolButton::Primary, Some([5, 10, 3]), None).unwrap();
        assert!(builder.drawing());

        // La altura se mide desde lo alto de B, (5.5, 11, 3.5): mirando 4 m más arriba, 4 bloques más.
        let eye = DVec3::new(5.5, 12.0, 13.5);
        let toward = |point: [f64; 3]| (DVec3::from_array(point) - eye).normalize().as_vec3().to_array();
        builder.aim(eye.to_array(), toward([5.5, 15.0, 3.5]), None, false);
        assert_eq!(builder.selection(), Some((IVec3::new(1, 10, -2), IVec3::new(5, 14, 3))));
        // Mirando por debajo de la base, crece hacia abajo.
        builder.aim(eye.to_array(), toward([5.5, 8.0, 3.5]), None, false);
        assert_eq!(builder.selection(), Some((IVec3::new(1, 7, -2), IVec3::new(5, 10, 3))));
        builder.aim(eye.to_array(), toward([5.5, 13.0, 3.5]), None, false);
        // Clic 3: confirma.
        builder.click(ToolButton::Primary, None, None).unwrap();
        assert!(!builder.drawing());
        assert_eq!(builder.selection(), Some((IVec3::new(1, 10, -2), IVec3::new(5, 12, 3))));
        assert!(builder.selection_text().ends_with("5x3x6 (90 blocks)"));

        assert_eq!(
            builder.region_op(RegionAction::Cut),
            Ok(PrefabOp::Cut { min: [1, 10, -2], max: [5, 12, 3] })
        );
        assert!(builder.save_op("  ").is_err());
        assert_eq!(
            builder.save_op(" trees/oak "),
            Ok(PrefabOp::Save { name: "trees/oak".into(), min: [1, 10, -2], max: [5, 12, 3] })
        );

        // La caja (con relleno, para verse a través del terreno) y sus dos esquinas.
        let overlay = builder.overlay(None, true);
        assert_eq!(overlay.outlines.len(), 9, "the box, six handles and two corners");
        assert!(overlay.outlines[0].fill > 0.0);
        assert_eq!(builder.overlay(None, false), Overlay::default());
        // Medidas: ancho, largo y alto, junto a la esquina de arriba más cercana.
        let labels = builder.labels([20.0, 30.0, 20.0]);
        assert_eq!(labels[0], ([3.5, 13.0, 4.0], "5".to_string()));
        assert_eq!(labels[1], ([6.0, 13.0, 1.0], "6".to_string()));
        assert_eq!(labels[2], ([6.0, 11.5, 4.0], "3".to_string()));

        // El clic derecho la borra.
        builder.click(ToolButton::Secondary, None, None).unwrap();
        assert_eq!(builder.corners(), (None, None));
        assert!(builder.labels([0.0; 3]).is_empty());
    }

    #[test]
    fn holding_the_first_click_draws_the_base_in_one_go() {
        let mut builder = Builder::default();
        builder.click(ToolButton::Primary, Some([0, 5, 0]), None).unwrap();
        // Con el botón pulsado la esquina B sigue al bloque apuntado.
        builder.aim([0.0; 3], [1.0, 0.0, 0.0], Some([4, 6, 7]), false);
        assert_eq!(builder.selection(), Some((IVec3::new(0, 5, 0), IVec3::new(4, 6, 7))));
        // Al soltar ya toca la altura; el siguiente clic la confirma.
        builder.release();
        assert!(builder.drawing());
        builder.click(ToolButton::Primary, None, None).unwrap();
        assert!(!builder.drawing());
        assert_eq!(builder.selection(), Some((IVec3::new(0, 5, 0), IVec3::new(4, 6, 7))));

        // Fuera de la caja, otro clic empieza una selección nueva.
        builder.aim([50.0, 5.5, 50.5], [1.0, 0.0, 0.0], Some([60, 5, 50]), false);
        assert_eq!(builder.hover_face(), None);
        builder.click(ToolButton::Primary, Some([60, 5, 50]), None).unwrap();
        assert_eq!(builder.corners(), (Some(IVec3::new(60, 5, 50)), Some(IVec3::new(60, 5, 50))));
    }

    #[test]
    fn placing_follows_the_aim_and_can_be_turned_and_raised() {
        let content = test_content(&["stone"]);
        let mut builder = Builder::default();
        assert!(builder.paste_op(Some([0, 0, 0])).is_err(), "nothing copied yet");

        builder.set_clipboard(clipboard(&content), "base:beam".into());
        builder.start_paste();
        assert!(builder.paste_op(None).is_err(), "nothing aimed at");
        // El anclaje (centro de la base: x = 1) cae en el hueco apuntado.
        let overlay = builder.overlay(Some([10, 5, 10]), false);
        assert_eq!(overlay.ghost.as_ref().map(|ghost| ghost.origin), Some([9, 5, 10]));
        assert_eq!(overlay.outlines[0].max, [12.0, 6.0, 11.0]);

        builder.rotate(&content);
        builder.lift(2);
        let overlay = builder.overlay(Some([10, 5, 10]), false);
        let ghost = overlay.ghost.expect("still placing");
        assert_eq!((ghost.prefab.size, ghost.origin), ([1, 1, 3], [10, 7, 9]));
        assert!(builder.clipboard_text().contains("base:beam (1x1x3, turned 90°)"));

        builder.paste_air = true;
        assert_eq!(
            builder.click(ToolButton::Secondary, Some([10, 4, 10]), Some([10, 5, 10])),
            Ok(Some(PrefabOp::Paste { pos: [10, 7, 10], turns: 1, air: true }))
        );
        assert!(builder.pasting(), "placing goes on until it is cancelled");
        assert_eq!(builder.click(ToolButton::Primary, None, None), Ok(None));
        assert!(!builder.pasting());
        assert!(builder.overlay(Some([10, 5, 10]), false).ghost.is_none());

        // Un portapapeles nuevo empieza sin girar ni subir.
        builder.set_clipboard(clipboard(&content), String::new());
        assert_eq!(builder.paste_op(Some([0, 0, 0])), Ok(PrefabOp::Paste { pos: [0, 0, 0], turns: 0, air: true }));
        assert!(builder.clipboard_text().contains("copied selection (3x1x1)"));
        assert_eq!(builder.hud_lines().len(), 4);
    }

    /// Selección de (0, 0, 0) a (3, 1, 3): la primera esquina es la mínima.
    fn boxed() -> Builder {
        let mut builder = Builder::default();
        builder.set_corner(true, IVec3::ZERO);
        builder.set_corner(false, IVec3::new(3, 1, 3));
        builder
    }

    #[test]
    fn the_sides_of_the_box_are_dragged_with_the_aim() {
        let mut builder = boxed();
        // Desde dentro, mirando al cielo: se apunta a la cara de arriba aunque no haya bloque.
        let eye = [2.0, 1.5, 2.0];
        builder.aim(eye, [0.0, 1.0, 0.0], None, false);
        assert_eq!(builder.hover_face(), Some(Face { axis: 1, positive: true }));
        let marks = builder.overlay(None, true).outlines;
        assert_eq!(marks.iter().filter(|mark| mark.color == HOT_COLOR).count(), 1, "its handle lights up");
        // Fuera de la manilla (la cara de arriba mide 4×4; la manilla, 1,6) no se agarra nada.
        builder.aim([3.5, 1.5, 3.5], [0.0, 1.0, 0.0], None, false);
        assert_eq!(builder.hover_face(), None);
        // La de abajo se agarra desde arriba, a través de la caja, sin teclas: basta con
        // no mirar justo en vertical (ahí la de arriba queda delante).
        let down = (DVec3::new(2.0, 0.0, 2.0) - DVec3::new(2.0, 9.0, 8.0)).normalize().as_vec3().to_array();
        builder.aim([2.0, 9.0, 8.0], down, Some([2, 1, 3]), false);
        assert_eq!(builder.hover_face(), Some(Face { axis: 1, positive: false }));

        // Desde fuera, de lado: la cara este, y se arrastra subiendo y bajando la mira.
        let eye = [10.0, 1.0, 2.0];
        builder.aim(eye, [-1.0, 0.0, 0.0], None, false);
        assert_eq!(builder.hover_face(), Some(Face { axis: 0, positive: true }));
        // Un bloque delante no la tapa (una cara enterrada se agarra igual), y con `far`
        // se elige la del otro lado.
        builder.aim(eye, [-1.0, 0.0, 0.0], Some([7, 1, 2]), false);
        assert_eq!(builder.hover_face(), Some(Face { axis: 0, positive: true }));
        builder.aim(eye, [-1.0, 0.0, 0.0], None, true);
        assert_eq!(builder.hover_face(), Some(Face { axis: 0, positive: false }));
        // Desde encima y mirando abajo, la cara inferior queda al otro lado.
        builder.aim([2.0, 9.0, 2.0], [0.0, -1.0, 0.0], Some([2, 5, 2]), true);
        assert_eq!(builder.hover_face(), Some(Face { axis: 1, positive: false }));

        // Arrastre de la cara de arriba desde fuera: se agarra en (2, 2, 4) mirando al norte.
        let eye = [2.0, 4.0, 10.0];
        let toward = |point: [f64; 3]| {
            let dir = (DVec3::from_array(point) - DVec3::from_array(eye)).normalize().as_vec3();
            dir.to_array()
        };
        builder.aim(eye, toward([2.0, 2.0, 2.0]), None, false);
        assert_eq!(builder.hover_face(), Some(Face { axis: 1, positive: true }));
        assert_eq!(builder.click(ToolButton::Primary, None, None), Ok(None));
        assert!(builder.dragging());
        // Subir la mira 5 m por encima del punto agarrado sube la cara 5 bloques.
        builder.aim(eye, toward([2.0, 7.0, 3.0]), None, false);
        assert_eq!(builder.selection(), Some((IVec3::ZERO, IVec3::new(3, 6, 3))));
        // Bajarla del todo no la deja cruzar el suelo de la caja.
        builder.aim(eye, toward([2.0, -9.0, 3.0]), None, false);
        assert_eq!(builder.selection(), Some((IVec3::ZERO, IVec3::new(3, 0, 3))));
        builder.aim(eye, toward([2.0, 4.0, 3.0]), None, false);
        builder.release();
        assert!(!builder.dragging());
        assert_eq!(builder.selection(), Some((IVec3::ZERO, IVec3::new(3, 3, 3))));
        assert_eq!(builder.corners().0, Some(IVec3::ZERO), "only the dragged side moved");
    }

    #[test]
    fn the_wheel_pushes_and_pulls_the_side_under_the_aim() {
        let mut builder = boxed();
        assert!(!builder.nudge(1), "not aiming at any side");
        // Cara oeste (la de −X) desde fuera: hacia fuera es restar.
        builder.aim([-5.0, 1.0, 2.0], [1.0, 0.0, 0.0], None, false);
        assert_eq!(builder.hover_face(), Some(Face { axis: 0, positive: false }));
        assert!(builder.nudge(2));
        assert_eq!(builder.selection(), Some((IVec3::new(-2, 0, 0), IVec3::new(3, 1, 3))));
        assert!(builder.nudge(-9));
        assert_eq!(builder.selection().map(|(min, _)| min.x), Some(3), "it stops at the opposite side");
        assert!(builder.nudge(500));
        assert_eq!(builder.selection().map(|(min, _)| min.x), Some(3 - 127), "and at the size limit");

        // Colocando un prefab no se tocan las caras.
        builder.start_paste();
        builder.aim([-200.0, 1.0, 2.0], [1.0, 0.0, 0.0], None, false);
        assert_eq!(builder.hover_face(), None);
    }
}
