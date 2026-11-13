//! Interfaces de usuario con Iced sobre una escena de KuraVoxel.
//!
//! El motor no conoce Iced: solo ofrece al juego el dispositivo gráfico y dos ganchos
//! (eventos de ventana y "dibuja encima de la escena"). Este crate pone lo que falta
//! entre esos ganchos y una interfaz de Iced, para que una pantalla sea solo esto:
//!
//! ```ignore
//! struct Counter { value: i32 }
//!
//! #[derive(Debug, Clone)]
//! enum Message { Increment }
//!
//! impl Screen for Counter {
//!     type Message = Message;
//!
//!     fn update(&mut self, message: Message) {
//!         match message { Message::Increment => self.value += 1 }
//!     }
//!
//!     fn view(&self) -> Element<'_, Message> {
//!         column![text(self.value), button("+").on_press(Message::Increment)].into()
//!     }
//! }
//! ```
//!
//! El juego guarda un [`IcedLayer`], le pasa los eventos de ventana y, en cada frame,
//! llama a [`IcedLayer::frame`] con la pantalla que quiera mostrar. Una pantalla no toca
//! la GPU ni la ventana: su estado y su `update` se prueban como cualquier otro código.
//!
//! En un mismo frame se pueden apilar varias pantallas, de abajo arriba: las de debajo
//! con [`IcedLayer::draw`] (solo se pintan, como un HUD) y la de encima con `frame`
//! (recibe la entrada). Cada tipo de pantalla conserva el estado de sus widgets.

use std::any::TypeId;
use std::collections::HashMap;
use std::time::Instant;

use iced_runtime::user_interface::{Cache, UserInterface};
use iced_wgpu::graphics::{Shell, Viewport};
use iced_wgpu::Engine;
use iced_winit::{conversion, Clipboard};
use kuravoxel_client::app::Host;
use winit::event::WindowEvent;
use winit::keyboard::ModifiersState;

pub use iced_wgpu::Renderer;
/// Tipos básicos de Iced (longitudes, colores, alineación...).
pub use iced_widget::core;
pub use iced_widget::core::Theme;
/// Todos los widgets de Iced (`button`, `column!`, `text_input`...).
pub use iced_widget as widget;

use core::widget::operation::focusable;
use core::{mouse, renderer, window, Event, Font, Pixels, Size};

/// Un árbol de widgets que emite mensajes de tipo `Message`.
pub type Element<'a, Message> = core::Element<'a, Message, Theme, Renderer>;
/// Nombre de un widget, para poder referirse a él desde fuera (darle el foco).
pub type WidgetId = core::widget::Id;

/// Tamaño de letra por defecto, en píxeles lógicos.
const DEFAULT_TEXT_SIZE: f32 = 16.0;
/// Eventos que se guardan como mucho entre dos frames (si nadie los consume, se tiran).
const MAX_PENDING_EVENTS: usize = 512;
/// Límites de la escala de la interfaz elegida por el usuario.
const UI_SCALE_RANGE: (f32, f32) = (0.5, 3.0);

/// Una pantalla: estado + mensajes + vista, al estilo Elm.
pub trait Screen {
    /// Lo que puede pasar en la pantalla (un clic, texto escrito...).
    type Message: std::fmt::Debug + Clone;

    /// Aplica un mensaje al estado.
    fn update(&mut self, message: Self::Message);

    /// Construye la interfaz a partir del estado actual.
    fn view(&self) -> Element<'_, Self::Message>;
}

/// Dibuja pantallas de Iced encima de la escena y les reparte la entrada.
pub struct IcedLayer {
    renderer: Renderer,
    viewport: Viewport,
    /// Tamaño de la ventana en píxeles y su escala; con `ui_scale` dan el `viewport`.
    physical_size: Size<u32>,
    window_scale: f32,
    /// Escala extra elegida por el usuario (1 = tamaño normal).
    ui_scale: f32,
    format: wgpu::TextureFormat,
    theme: Theme,
    /// Estado interno de los widgets (foco, desplazamiento...) de cada tipo de pantalla,
    /// entre frames.
    caches: HashMap<TypeId, Cache>,
    cursor: mouse::Cursor,
    modifiers: ModifiersState,
    clipboard: Clipboard,
    /// Eventos recibidos desde el último frame, ya convertidos a los de Iced.
    events: Vec<Event>,
    /// Widget que debe recibir el foco en el próximo `frame`.
    focus: Option<WidgetId>,
}

impl IcedLayer {
    pub fn new(host: &Host) -> Self {
        let gpu = host.gpu;
        let engine = Engine::new(
            gpu.adapter,
            gpu.device.clone(),
            gpu.queue.clone(),
            gpu.format,
            None,
            Shell::headless(),
        );
        let size = host.window.inner_size();
        let physical_size = Size::new(size.width.max(1), size.height.max(1));
        let window_scale = host.window.scale_factor() as f32;
        Self {
            renderer: Renderer::new(engine, Font::default(), Pixels(DEFAULT_TEXT_SIZE)),
            viewport: Viewport::with_physical_size(physical_size, window_scale),
            physical_size,
            window_scale,
            ui_scale: 1.0,
            format: gpu.format,
            theme: Theme::Dark,
            caches: HashMap::new(),
            cursor: mouse::Cursor::Unavailable,
            modifiers: ModifiersState::default(),
            clipboard: Clipboard::connect(host.window.clone()),
            events: Vec::new(),
            focus: None,
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Agranda o encoge toda la interfaz (1 = tamaño normal).
    pub fn set_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(UI_SCALE_RANGE.0, UI_SCALE_RANGE.1);
        self.update_viewport();
    }

    /// Da el foco del teclado a un widget (una caja de texto) en el próximo `frame`.
    pub fn focus(&mut self, id: impl Into<WidgetId>) {
        self.focus = Some(id.into());
    }

    fn update_viewport(&mut self) {
        self.viewport =
            Viewport::with_physical_size(self.physical_size, self.window_scale * self.ui_scale);
    }

    /// Recibe un evento de ventana. Hay que pasarlos todos, haya o no pantalla visible,
    /// para que tamaño, escala y posición del cursor estén siempre al día.
    pub fn window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                self.physical_size = Size::new(size.width.max(1), size.height.max(1));
                self.update_viewport();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.window_scale = *scale_factor as f32;
                self.update_viewport();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = mouse::Cursor::Available(conversion::cursor_position(
                    *position,
                    self.viewport.scale_factor(),
                ));
            }
            WindowEvent::CursorLeft { .. } => self.cursor = mouse::Cursor::Unavailable,
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            _ => {}
        }
        let scale = self.viewport.scale_factor();
        if let Some(event) = conversion::window_event(event.clone(), scale, self.modifiers) {
            if self.events.len() >= MAX_PENDING_EVENTS {
                self.events.clear();
            }
            self.events.push(event);
        }
    }

    /// Descarta la entrada acumulada. Para los frames en los que ninguna pantalla la
    /// recibe (no se llama a `frame`).
    pub fn discard_events(&mut self) {
        self.events.clear();
    }

    /// Pinta una pantalla sobre `target` sin darle entrada ni cursor: para lo que solo se
    /// mira (un HUD) o queda debajo de la pantalla que sí recibe la entrada.
    pub fn draw<S: Screen + 'static>(&mut self, screen: &S, target: &wgpu::TextureView) {
        let key = TypeId::of::<S>();
        let cache = self.caches.remove(&key).unwrap_or_default();
        let mut interface =
            UserInterface::build(screen.view(), self.viewport.logical_size(), cache, &mut self.renderer);
        let style = renderer::Style { text_color: self.theme.palette().text };
        interface.draw(&mut self.renderer, &self.theme, &style, mouse::Cursor::Unavailable);
        self.caches.insert(key, interface.into_cache());

        self.renderer.present(None, self.format, target, &self.viewport);
    }

    /// Un frame de una pantalla: le entrega la entrada pendiente, aplica los mensajes
    /// que resulten y la dibuja sobre `target` (la escena ya pintada).
    pub fn frame<S: Screen + 'static>(&mut self, screen: &mut S, target: &wgpu::TextureView) {
        let key = TypeId::of::<S>();
        let bounds = self.viewport.logical_size();
        // Los widgets animados (el cursor de una caja de texto) avanzan con este evento.
        self.events
            .push(Event::Window(window::Event::RedrawRequested(Instant::now())));

        let mut messages = Vec::new();
        let mut interface = UserInterface::build(
            screen.view(),
            bounds,
            self.caches.remove(&key).unwrap_or_default(),
            &mut self.renderer,
        );
        if let Some(id) = self.focus.take() {
            interface.operate(&self.renderer, &mut focusable::focus::<()>(id));
        }
        let _ = interface.update(
            &self.events,
            self.cursor,
            &mut self.renderer,
            &mut self.clipboard,
            &mut messages,
        );
        self.events.clear();

        // Si la entrada ha cambiado el estado, la vista se reconstruye antes de dibujar:
        // lo que se ve siempre corresponde al estado actual.
        if !messages.is_empty() {
            let cache = interface.into_cache();
            for message in messages {
                screen.update(message);
            }
            interface = UserInterface::build(screen.view(), bounds, cache, &mut self.renderer);
        }

        let style = renderer::Style { text_color: self.theme.palette().text };
        interface.draw(&mut self.renderer, &self.theme, &style, self.cursor);
        self.caches.insert(key, interface.into_cache());

        self.renderer.present(None, self.format, target, &self.viewport);
    }
}
