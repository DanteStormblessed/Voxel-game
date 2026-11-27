//! Lienzo de nodos del editor de biomas: cajas con puertos, cables entre ellas y la
//! miniatura del valor de cada nodo.
//!
//! El lienzo no sabe de archivos ni de biomas: dibuja lo que le dan (`GraphNode`) y
//! cuenta lo que hace el ratón (`GraphEvent`): elegir, mover, estirar, unir, soltar,
//! desplazar y acercar. Mientras se arrastra algo, la posición (o el tamaño) provisional
//! vive en el propio lienzo; la pantalla solo se entera al soltar, así un arrastre no
//! reescribe el archivo en cada movimiento.
//!
//! Cada caja se puede estirar desde su esquina inferior derecha: más ancha, los textos
//! largos se leen enteros; más alta, la miniatura crece. Un doble clic en la esquina la
//! devuelve a su tamaño normal.

use std::time::{Duration, Instant};

use kuravoxel_ui::core::keyboard::{self, key::Named, Key};
use kuravoxel_ui::core::{mouse, Color, Pixels, Point, Rectangle, Size, Vector};
use kuravoxel_ui::widget::canvas::{self, Event, Frame, Geometry, Path, Stroke, Text};
use kuravoxel_ui::widget::image;
use kuravoxel_ui::{Renderer, Theme};

/// Medidas de una caja, en unidades del lienzo (píxeles con zoom 1).
pub const NODE_WIDTH: f32 = 176.0;
/// Lo más estrecha y lo más grande que puede quedar una caja estirada a mano.
pub const MIN_WIDTH: f32 = 140.0;
const MAX_SIZE: [f32; 2] = [900.0, 1400.0];
const HEADER: f32 = 40.0;
/// Fila de una entrada (con su puerto).
const ROW: f32 = 20.0;
/// Fila de un parámetro (sin puerto: `scale: 900`).
const PARAM_ROW: f32 = 16.0;
const THUMB: f32 = 80.0;
const MIN_THUMB: f32 = 40.0;
const FOOTER: f32 = 18.0;
/// Lado de la esquina por la que se estira una caja.
const GRIP: f32 = 14.0;
const PORT_RADIUS: f32 = 5.0;
/// Distancia (en píxeles de pantalla) a la que el ratón alcanza un puerto.
const PORT_REACH: f32 = 10.0;
/// Dos clics en la esquina con menos de esto entre ellos devuelven el tamaño normal.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Ancho medio de una letra respecto a su tamaño, para recortar los textos a la caja.
const CHAR_WIDTH: f32 = 0.56;
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 2.0;

const BACKGROUND: Color = Color::from_rgba(0.07, 0.08, 0.1, 0.92);
const GRID: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.05);
const BOX: Color = Color::from_rgb(0.17, 0.18, 0.22);
const BOX_HEADER: Color = Color::from_rgb(0.23, 0.25, 0.31);
const TEXT: Color = Color::from_rgb(0.92, 0.93, 0.95);
const MUTED: Color = Color::from_rgb(0.62, 0.65, 0.7);
const WIRE: Color = Color::from_rgb(0.62, 0.78, 0.95);
const SELECTED: Color = Color::WHITE;
const ROOT: Color = Color::from_rgb(0.98, 0.78, 0.25);
const ERROR: Color = Color::from_rgb(1.0, 0.42, 0.38);

/// Una entrada de un nodo: un puerto a la izquierda.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Texto de la fila (`a: 0.5`, `inputs`).
    pub label: String,
    pub param: String,
    /// Posición en una lista de entradas (`inputs`); la fila "+" es la del final.
    pub index: Option<usize>,
    /// Nodo de este lienzo conectado aquí (se dibuja el cable).
    pub link: Option<String>,
}

/// Un nodo tal como se dibuja.
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub name: String,
    /// Segunda línea de la cabecera: el tipo del nodo.
    pub kind: String,
    pub pos: [f32; 2],
    /// Tamaño elegido a mano (ancho, alto); `None` = el normal.
    pub size: Option<[f32; 2]>,
    pub rows: Vec<Row>,
    /// Los parámetros que no son entradas, ya escritos (`scale: 300, 1200`).
    pub params: Vec<String>,
    pub thumbnail: Option<image::Handle>,
    /// Bajo la miniatura: el rango de valores, o el error.
    pub footer: String,
    pub error: bool,
    /// Es la salida del bioma (su altura).
    pub root: bool,
}

/// Medidas de una caja ya resueltas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub width: f32,
    pub height: f32,
    /// Lado de la miniatura.
    pub thumb: f32,
}

impl GraphNode {
    /// Alto de todo menos la miniatura.
    fn fixed_height(&self) -> f32 {
        let params = if self.params.is_empty() { 0.0 } else { PARAM_ROW * self.params.len() as f32 + 4.0 };
        HEADER + ROW * self.rows.len() as f32 + params + 6.0 + FOOTER
    }

    /// Lo más baja que puede quedar la caja.
    pub fn min_height(&self) -> f32 {
        self.fixed_height() + MIN_THUMB
    }

    /// Medidas con un tamaño dado (el elegido a mano, o el normal con `None`).
    pub fn shape_with(&self, size: Option<[f32; 2]>) -> Shape {
        let fixed = self.fixed_height();
        match size {
            None => Shape { width: NODE_WIDTH, height: fixed + THUMB, thumb: THUMB },
            Some([w, h]) => {
                let width = w.clamp(MIN_WIDTH, MAX_SIZE[0]);
                let height = h.clamp(self.min_height(), MAX_SIZE[1].max(self.min_height()));
                let thumb = (height - fixed).min(width - 20.0).max(MIN_THUMB);
                Shape { width, height, thumb }
            }
        }
    }

    pub fn shape(&self) -> Shape {
        self.shape_with(self.size)
    }

    pub fn height(&self) -> f32 {
        self.shape().height
    }

    fn params_top(&self) -> f32 {
        HEADER + ROW * self.rows.len() as f32 + 2.0
    }

    fn thumb_top(&self) -> f32 {
        let params = if self.params.is_empty() { 0.0 } else { PARAM_ROW * self.params.len() as f32 + 4.0 };
        HEADER + ROW * self.rows.len() as f32 + params + 6.0
    }
}

fn input_port(pos: [f32; 2], row: usize) -> Point {
    Point::new(pos[0], pos[1] + HEADER + ROW * row as f32 + ROW / 2.0)
}

fn output_port(pos: [f32; 2], width: f32) -> Point {
    Point::new(pos[0] + width, pos[1] + 20.0)
}

/// Recorta un texto para que quepa en `room` unidades con letra de tamaño `size`.
pub fn fit(text: &str, room: f32, size: f32) -> String {
    let fits = (room / (size * CHAR_WIDTH)).floor().max(1.0) as usize;
    if text.chars().count() <= fits {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(fits.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Lo que ha hecho el ratón en el lienzo.
#[derive(Clone, Debug, PartialEq)]
pub enum GraphEvent {
    Select(Option<String>),
    /// Un nodo soltado en otra posición.
    Move(String, [f32; 2]),
    /// Una caja estirada a otro tamaño (`None` = vuelve al normal).
    Resize(String, Option<[f32; 2]>),
    /// Un cable de la salida de `from` a una entrada de `to`.
    Connect { from: String, to: String, param: String, index: Option<usize> },
    /// Un cable quitado de una entrada.
    Disconnect { node: String, param: String, index: Option<usize> },
    /// Nueva vista: esquina superior izquierda (en unidades del lienzo) y zoom.
    View([f32; 2], f32),
    Delete(String),
}

/// El lienzo de un frame.
pub struct NodeGraph {
    pub nodes: Vec<GraphNode>,
    pub selected: Option<String>,
    pub pan: [f32; 2],
    pub zoom: f32,
}

#[derive(Debug, Default)]
pub enum Drag {
    #[default]
    None,
    Node { name: String, grab: Vector, pos: [f32; 2] },
    /// Estirando una caja desde su esquina: punto del lienzo donde empezó, tamaño de
    /// entonces y tamaño provisional.
    Resize { name: String, anchor: Point, start: [f32; 2], size: [f32; 2] },
    Pan { last: Point, pan: [f32; 2] },
    Wire { from: String, at: Point },
}

#[derive(Debug, Default)]
pub struct GraphState {
    drag: Drag,
    /// Último clic en la esquina de una caja (para el doble clic).
    last_grip: Option<(String, Instant)>,
}

/// Qué hay bajo el ratón.
#[derive(Debug, PartialEq)]
enum Hit {
    Output(usize),
    Input(usize, usize),
    /// La esquina por la que se estira la caja.
    Grip(usize),
    Body(usize),
    Empty,
}

impl NodeGraph {
    fn to_screen(&self, pan: [f32; 2], point: Point) -> Point {
        Point::new((point.x - pan[0]) * self.zoom, (point.y - pan[1]) * self.zoom)
    }

    fn to_graph(&self, pan: [f32; 2], point: Point) -> Point {
        Point::new(point.x / self.zoom + pan[0], point.y / self.zoom + pan[1])
    }

    /// Posición de un nodo, con la provisional si se está arrastrando.
    fn pos_of(&self, state: &GraphState, index: usize) -> [f32; 2] {
        match &state.drag {
            Drag::Node { name, pos, .. } if *name == self.nodes[index].name => *pos,
            _ => self.nodes[index].pos,
        }
    }

    /// Medidas de un nodo, con las provisionales si se está estirando.
    fn shape_of(&self, state: &GraphState, index: usize) -> Shape {
        let node = &self.nodes[index];
        match &state.drag {
            Drag::Resize { name, size, .. } if *name == node.name => node.shape_with(Some(*size)),
            _ => node.shape(),
        }
    }

    fn rect_of(&self, state: &GraphState, index: usize) -> Rectangle {
        let [x, y] = self.pos_of(state, index);
        let shape = self.shape_of(state, index);
        Rectangle::new(Point::new(x, y), Size::new(shape.width, shape.height))
    }

    fn pan_of(&self, state: &GraphState) -> [f32; 2] {
        match &state.drag {
            Drag::Pan { pan, .. } => *pan,
            _ => self.pan,
        }
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.name == name)
    }

    /// Qué hay en un punto de la pantalla (relativo al lienzo). Los nodos de encima (los
    /// últimos) mandan.
    fn hit(&self, state: &GraphState, cursor: Point) -> Hit {
        let pan = self.pan_of(state);
        // El puerto más cercano al alcance del ratón; si no hay, el nodo de encima (los
        // últimos se dibujan encima).
        let mut best: Option<(f32, Hit)> = None;
        let mut consider = |port: Point, hit: Hit| {
            let distance = self.to_screen(pan, port).distance(cursor);
            if distance <= PORT_REACH && best.as_ref().is_none_or(|(d, _)| distance < *d) {
                best = Some((distance, hit));
            }
        };
        for index in 0..self.nodes.len() {
            let pos = self.pos_of(state, index);
            consider(output_port(pos, self.shape_of(state, index).width), Hit::Output(index));
            for row in 0..self.nodes[index].rows.len() {
                consider(input_port(pos, row), Hit::Input(index, row));
            }
        }
        if let Some((_, hit)) = best {
            return hit;
        }
        let point = self.to_graph(pan, cursor);
        for index in (0..self.nodes.len()).rev() {
            let rect = self.rect_of(state, index);
            if rect.contains(point) {
                // La esquina mide lo mismo en pantalla con cualquier zoom.
                let grip = GRIP / self.zoom.max(0.5);
                let corner = Point::new(rect.x + rect.width, rect.y + rect.height);
                return if corner.x - point.x <= grip && corner.y - point.y <= grip {
                    Hit::Grip(index)
                } else {
                    Hit::Body(index)
                };
            }
        }
        Hit::Empty
    }
}

impl canvas::Program<GraphEvent, Theme, Renderer> for NodeGraph {
    type State = GraphState;

    fn update(
        &self,
        state: &mut GraphState,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<GraphEvent>> {
        let inside = cursor.position_in(bounds);
        let action = |event: GraphEvent| Some(canvas::Action::publish(event).and_capture());
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let point = inside?;
                if matches!(button, mouse::Button::Right | mouse::Button::Middle) {
                    state.drag = Drag::Pan { last: point, pan: self.pan };
                    return Some(canvas::Action::capture());
                }
                if *button != mouse::Button::Left {
                    return None;
                }
                let pan = self.pan;
                match self.hit(state, point) {
                    Hit::Output(index) => {
                        state.drag = Drag::Wire { from: self.nodes[index].name.clone(), at: point };
                        Some(canvas::Action::capture())
                    }
                    Hit::Input(index, row) => {
                        // Agarrar un cable por su punta lo suelta de esa entrada y se
                        // lleva su otro extremo, para conectarlo en otro sitio.
                        let node = &self.nodes[index];
                        let row = &node.rows[row];
                        let link = row.link.clone()?;
                        state.drag = Drag::Wire { from: link, at: point };
                        action(GraphEvent::Disconnect {
                            node: node.name.clone(),
                            param: row.param.clone(),
                            index: row.index,
                        })
                    }
                    Hit::Grip(index) => {
                        let name = self.nodes[index].name.clone();
                        let now = Instant::now();
                        let double = state
                            .last_grip
                            .as_ref()
                            .is_some_and(|(last, at)| *last == name && now.duration_since(*at) <= DOUBLE_CLICK);
                        if double {
                            state.last_grip = None;
                            return action(GraphEvent::Resize(name, None));
                        }
                        state.last_grip = Some((name.clone(), now));
                        let shape = self.nodes[index].shape();
                        let start = [shape.width, shape.height];
                        let anchor = self.to_graph(pan, point);
                        state.drag = Drag::Resize { name: name.clone(), anchor, start, size: start };
                        action(GraphEvent::Select(Some(name)))
                    }
                    Hit::Body(index) => {
                        let pos = self.nodes[index].pos;
                        let grab = self.to_graph(pan, point) - Point::new(pos[0], pos[1]);
                        state.drag = Drag::Node { name: self.nodes[index].name.clone(), grab, pos };
                        action(GraphEvent::Select(Some(self.nodes[index].name.clone())))
                    }
                    Hit::Empty => {
                        state.drag = Drag::Pan { last: point, pan };
                        action(GraphEvent::Select(None))
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let point = cursor.position_from(bounds.position())?;
                match &mut state.drag {
                    Drag::None => None,
                    Drag::Node { grab, pos, .. } => {
                        let at = self.to_graph(self.pan, point) - *grab;
                        *pos = [at.x, at.y];
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    Drag::Resize { name, anchor, start, size } => {
                        let delta = self.to_graph(self.pan, point) - *anchor;
                        let min = self.index(name).map_or(0.0, |i| self.nodes[i].min_height());
                        *size = [
                            (start[0] + delta.x).clamp(MIN_WIDTH, MAX_SIZE[0]),
                            (start[1] + delta.y).clamp(min, MAX_SIZE[1].max(min)),
                        ];
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    Drag::Pan { last, pan } => {
                        let delta = point - *last;
                        *last = point;
                        pan[0] -= delta.x / self.zoom;
                        pan[1] -= delta.y / self.zoom;
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    Drag::Wire { at, .. } => {
                        *at = point;
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(_)) => match std::mem::take(&mut state.drag) {
                Drag::None => None,
                Drag::Node { name, pos, .. } => {
                    let start = self.index(&name).map(|i| self.nodes[i].pos);
                    if start == Some(pos) {
                        return Some(canvas::Action::capture());
                    }
                    action(GraphEvent::Move(name, [pos[0].round(), pos[1].round()]))
                }
                Drag::Resize { name, start, size, .. } => {
                    if size == start {
                        return Some(canvas::Action::capture());
                    }
                    action(GraphEvent::Resize(name, Some([size[0].round(), size[1].round()])))
                }
                Drag::Pan { pan, .. } => action(GraphEvent::View(pan, self.zoom)),
                Drag::Wire { from, at } => match self.hit(state, at) {
                    Hit::Input(index, row) if self.nodes[index].name != from => {
                        let node = &self.nodes[index];
                        action(GraphEvent::Connect {
                            from,
                            to: node.name.clone(),
                            param: node.rows[row].param.clone(),
                            index: node.rows[row].index,
                        })
                    }
                    _ => Some(canvas::Action::request_redraw().and_capture()),
                },
            },
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let point = inside?;
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                let zoom = (self.zoom * 1.15f32.powf(lines)).clamp(MIN_ZOOM, MAX_ZOOM);
                // El punto bajo el ratón se queda quieto.
                let anchor = self.to_graph(self.pan, point);
                let pan = [anchor.x - point.x / zoom, anchor.y - point.y / zoom];
                action(GraphEvent::View(pan, zoom))
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key: Key::Named(Named::Delete), .. }) => {
                inside?;
                action(GraphEvent::Delete(self.selected.clone()?))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &GraphState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let area = Rectangle::with_size(bounds.size());
        frame.with_clip(area, |frame| self.draw_graph(frame, state, area, cursor.position_in(bounds)));
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &GraphState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match &state.drag {
            Drag::Node { .. } | Drag::Pan { .. } => return mouse::Interaction::Grabbing,
            Drag::Resize { .. } => return mouse::Interaction::ResizingDiagonallyDown,
            Drag::Wire { .. } => return mouse::Interaction::Crosshair,
            Drag::None => {}
        }
        match cursor.position_in(bounds).map(|point| self.hit(state, point)) {
            Some(Hit::Output(_) | Hit::Input(..)) => mouse::Interaction::Crosshair,
            Some(Hit::Grip(_)) => mouse::Interaction::ResizingDiagonallyDown,
            Some(Hit::Body(_)) => mouse::Interaction::Grab,
            _ => mouse::Interaction::default(),
        }
    }
}

fn text(content: impl Into<String>, position: Point, size: f32, color: Color) -> Text {
    Text { content: content.into(), position, size: Pixels(size), color, ..Text::default() }
}

/// Cable de una salida a una entrada: una curva que sale y entra en horizontal.
fn wire(from: Point, to: Point) -> Path {
    let bend = ((to.x - from.x).abs() * 0.5).max(40.0);
    Path::new(|path| {
        path.move_to(from);
        path.bezier_curve_to(Point::new(from.x + bend, from.y), Point::new(to.x - bend, to.y), to);
    })
}

impl NodeGraph {
    fn draw_graph(&self, frame: &mut Frame, state: &GraphState, area: Rectangle, cursor: Option<Point>) {
        let pan = self.pan_of(state);
        let zoom = self.zoom;
        frame.fill_rectangle(Point::ORIGIN, area.size(), BACKGROUND);

        // Rejilla cada 40 unidades.
        let step = 40.0 * zoom;
        if step >= 8.0 {
            let offset = Point::new((-pan[0] * zoom).rem_euclid(step), (-pan[1] * zoom).rem_euclid(step));
            let mut x = offset.x;
            while x < area.width {
                frame.fill_rectangle(Point::new(x, 0.0), Size::new(1.0, area.height), GRID);
                x += step;
            }
            let mut y = offset.y;
            while y < area.height {
                frame.fill_rectangle(Point::new(0.0, y), Size::new(area.width, 1.0), GRID);
                y += step;
            }
        }

        let screen = |point: Point| self.to_screen(pan, point);
        let wire_stroke = Stroke::default().with_color(WIRE).with_width((2.0 * zoom).max(1.0));

        // Cables, por debajo de las cajas.
        for (index, node) in self.nodes.iter().enumerate() {
            let pos = self.pos_of(state, index);
            for (row, entry) in node.rows.iter().enumerate() {
                let Some(source) = entry.link.as_deref().and_then(|name| self.index(name)) else { continue };
                let from = screen(output_port(self.pos_of(state, source), self.shape_of(state, source).width));
                frame.stroke(&wire(from, screen(input_port(pos, row))), wire_stroke);
            }
        }

        let hovered = cursor.map(|point| self.hit(state, point));
        for (index, node) in self.nodes.iter().enumerate() {
            let pos = self.pos_of(state, index);
            let shape = self.shape_of(state, index);
            let rect = self.rect_of(state, index);
            let top_left = screen(rect.position());
            let size = Size::new(rect.width * zoom, rect.height * zoom);
            if top_left.x > area.width || top_left.y > area.height || top_left.x + size.width < 0.0 || top_left.y + size.height < 0.0 {
                continue;
            }
            let radius = 6.0 * zoom;
            frame.fill(&Path::rounded_rectangle(top_left, size, radius.into()), BOX);
            frame.fill(
                &Path::rounded_rectangle(top_left, Size::new(size.width, HEADER * zoom), radius.into()),
                BOX_HEADER,
            );
            let border = if node.error {
                Some(ERROR)
            } else if self.selected.as_deref() == Some(node.name.as_str()) {
                Some(SELECTED)
            } else if node.root {
                Some(ROOT)
            } else {
                None
            };
            if let Some(color) = border {
                let stroke = Stroke::default().with_color(color).with_width((2.0 * zoom).max(1.0));
                frame.stroke(&Path::rounded_rectangle(top_left, size, radius.into()), stroke);
            }

            let at = |x: f32, y: f32| screen(Point::new(pos[0] + x, pos[1] + y));
            // Los textos se recortan al ancho de la caja (en unidades del lienzo: el zoom
            // agranda igual la letra y la caja).
            let room = shape.width - 22.0;
            if zoom >= 0.4 {
                let name_color = if node.root { ROOT } else { TEXT };
                frame.fill_text(text(fit(&node.name, room, 14.0), at(10.0, 5.0), 14.0 * zoom, name_color));
                frame.fill_text(text(fit(&node.kind, room, 11.0), at(10.0, 22.0), 11.0 * zoom, MUTED));
            }

            // Entradas.
            for (row, entry) in node.rows.iter().enumerate() {
                let port = screen(input_port(pos, row));
                let lit = hovered == Some(Hit::Input(index, row)) || entry.link.is_some();
                let circle = Path::circle(port, PORT_RADIUS * zoom);
                if lit {
                    frame.fill(&circle, WIRE);
                } else {
                    frame.fill(&circle, BOX);
                    frame.stroke(&circle, Stroke::default().with_color(MUTED).with_width(1.0));
                }
                if zoom >= 0.45 {
                    let y = HEADER + ROW * row as f32 + 3.0;
                    frame.fill_text(text(fit(&entry.label, room, 12.0), at(12.0, y), 12.0 * zoom, TEXT));
                }
            }

            // Parámetros (sin puerto).
            if zoom >= 0.45 {
                for (line, param) in node.params.iter().enumerate() {
                    let y = node.params_top() + PARAM_ROW * line as f32;
                    frame.fill_text(text(fit(param, shape.width - 18.0, 11.0), at(10.0, y), 11.0 * zoom, MUTED));
                }
            }

            // Salida.
            let output = Path::circle(screen(output_port(pos, shape.width)), (PORT_RADIUS + 1.0) * zoom);
            let output_color = if hovered == Some(Hit::Output(index)) { SELECTED } else if node.root { ROOT } else { WIRE };
            frame.fill(&output, output_color);

            // Miniatura y rango.
            let thumb_top = node.thumb_top();
            let thumb_left = (shape.width - shape.thumb) / 2.0;
            let thumb = Rectangle::new(at(thumb_left, thumb_top), Size::new(shape.thumb * zoom, shape.thumb * zoom));
            match &node.thumbnail {
                Some(handle) => frame.draw_image(thumb, canvas::Image::new(handle.clone()).filter_method(image::FilterMethod::Nearest)),
                None => frame.fill_rectangle(thumb.position(), thumb.size(), Color::from_rgb(0.1, 0.1, 0.12)),
            }
            if zoom >= 0.45 {
                let color = if node.error { ERROR } else { MUTED };
                let footer = fit(&node.footer, shape.width - 18.0, 11.0);
                frame.fill_text(text(footer, at(10.0, thumb_top + shape.thumb + 2.0), 11.0 * zoom, color));
            }

            // Esquina para estirar la caja: dos rayas en diagonal.
            let grip_color = match (&state.drag, hovered.as_ref()) {
                (Drag::Resize { name, .. }, _) if *name == node.name => SELECTED,
                (_, Some(Hit::Grip(i))) if *i == index => SELECTED,
                _ => MUTED,
            };
            let corner = Point::new(pos[0] + shape.width - 3.0, pos[1] + shape.height - 3.0);
            let grip = Path::new(|path| {
                for offset in [5.0, 10.0] {
                    path.move_to(screen(Point::new(corner.x - offset, corner.y)));
                    path.line_to(screen(Point::new(corner.x, corner.y - offset)));
                }
            });
            frame.stroke(&grip, Stroke::default().with_color(grip_color).with_width((1.5 * zoom).max(1.0)));
        }

        // Cable que se está tendiendo.
        if let Drag::Wire { from, at } = &state.drag {
            if let Some(source) = self.index(from) {
                let start = screen(output_port(self.pos_of(state, source), self.shape_of(state, source).width));
                frame.stroke(&wire(start, *at), wire_stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, pos: [f32; 2], rows: usize) -> GraphNode {
        GraphNode {
            name: name.into(),
            kind: "add".into(),
            pos,
            size: None,
            rows: (0..rows)
                .map(|i| Row { label: format!("in {i}"), param: "inputs".into(), index: Some(i), link: None })
                .collect(),
            params: vec!["scale: 900".into()],
            thumbnail: None,
            footer: String::new(),
            error: false,
            root: false,
        }
    }

    fn graph(nodes: Vec<GraphNode>) -> NodeGraph {
        NodeGraph { nodes, selected: None, pan: [-20.0, -10.0], zoom: 0.5 }
    }

    /// Un evento del ratón en un punto del lienzo (en unidades del lienzo); devuelve lo
    /// que publica.
    fn mouse(graph: &NodeGraph, state: &mut GraphState, event: mouse::Event, at: Point) -> Option<GraphEvent> {
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(1000.0, 800.0));
        let cursor = mouse::Cursor::Available(graph.to_screen(graph.pan, at));
        let action = canvas::Program::update(graph, state, &Event::Mouse(event), bounds, cursor)?;
        action.into_inner().0
    }

    #[test]
    fn ports_and_bodies_are_found_under_the_cursor() {
        let graph = graph(vec![node("a", [0.0, 0.0], 2), node("b", [300.0, 0.0], 3)]);
        let state = GraphState::default();
        let on_screen = |point: Point| graph.to_screen(graph.pan, point);
        assert_eq!(graph.hit(&state, on_screen(output_port([0.0, 0.0], NODE_WIDTH))), Hit::Output(0));
        assert_eq!(graph.hit(&state, on_screen(input_port([300.0, 0.0], 2))), Hit::Input(1, 2));
        assert_eq!(graph.hit(&state, on_screen(Point::new(350.0, 150.0))), Hit::Body(1));
        assert_eq!(graph.hit(&state, on_screen(Point::new(250.0, 150.0))), Hit::Empty);
        let height = graph.nodes[1].height();
        assert_eq!(graph.hit(&state, on_screen(Point::new(300.0 + NODE_WIDTH - 3.0, height - 3.0))), Hit::Grip(1));
        // Ida y vuelta entre pantalla y lienzo.
        let point = Point::new(123.0, -45.0);
        let back = graph.to_graph(graph.pan, on_screen(point));
        assert!((back.x - point.x).abs() < 1e-3 && (back.y - point.y).abs() < 1e-3);
    }

    #[test]
    fn a_box_is_stretched_from_its_corner_and_reset_with_a_double_click() {
        let mut graph = graph(vec![node("a", [0.0, 0.0], 2)]);
        let mut state = GraphState::default();
        let normal = graph.nodes[0].shape();
        let corner = Point::new(normal.width - 3.0, normal.height - 3.0);
        assert_eq!(mouse(&graph, &mut state, mouse::Event::ButtonPressed(mouse::Button::Left), corner), Some(GraphEvent::Select(Some("a".into()))));
        let moved = Point::new(corner.x + 120.0, corner.y + 200.0);
        mouse(&graph, &mut state, mouse::Event::CursorMoved { position: Point::ORIGIN }, moved);
        let stretched = graph.shape_of(&state, 0);
        assert_eq!(stretched.width, normal.width + 120.0);
        assert!(stretched.thumb > normal.thumb, "the thumbnail grows with the box");
        let released = mouse(&graph, &mut state, mouse::Event::ButtonReleased(mouse::Button::Left), moved);
        assert_eq!(released, Some(GraphEvent::Resize("a".into(), Some([normal.width + 120.0, normal.height + 200.0]))));

        // Nunca más pequeña que su contenido.
        graph.nodes[0].size = Some([10.0, 10.0]);
        let tiny = graph.nodes[0].shape();
        assert_eq!((tiny.width, tiny.height), (MIN_WIDTH, graph.nodes[0].min_height()));

        // Doble clic en la esquina: tamaño normal.
        graph.nodes[0].size = None;
        state = GraphState::default();
        mouse(&graph, &mut state, mouse::Event::ButtonPressed(mouse::Button::Left), corner);
        mouse(&graph, &mut state, mouse::Event::ButtonReleased(mouse::Button::Left), corner);
        let again = mouse(&graph, &mut state, mouse::Event::ButtonPressed(mouse::Button::Left), corner);
        assert_eq!(again, Some(GraphEvent::Resize("a".into(), None)));
    }

    #[test]
    fn long_texts_are_cut_to_the_box() {
        assert_eq!(fit("scale: 900", 200.0, 11.0), "scale: 900");
        let cut = fit("points: [-1, 0], [0, 0.5], [1, 1], [2, 4]", 80.0, 11.0);
        assert!(cut.ends_with('…') && cut.chars().count() < 20, "{cut}");
    }
}
