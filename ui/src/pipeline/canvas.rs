//! Lienzo del pipeline: `gtk::DrawingArea` con gestos (§6, A9).
//!
//! La capa widget de [`crate::pipeline::canvas_model`] (el modelo puro con
//! la transformada única, la clasificación de aristas y los hit-tests vive
//! ahí). Aquí se añade:
//!
//! - gestos: clic (selección de paso/checkbox/arista), drag de nodo con
//!   persistencia SOLO del nodo movido (`store.set_layout`), pan (arrastrar
//!   el vacío), zoom con ctrl+rueda anclado al cursor, arrastre nodo→nodo
//!   para conectar y reconectar por el extremo de destino (remove+add);
//! - Delete/Backspace sobre una arista de dependencia seleccionada (A9);
//! - refresco animado del borde ámbar mientras hay pasos `running` (timer
//!   propio con `Weak` — al morir el estado el timer se auto-detiene,
//!   §11 A12);
//! - estados vacíos del §6 (sin proyecto / sin pipeline).
//!
//! Ninguna mutación estructural ocurre si `edits_enabled` es `false`
//! (§11 A12: en ventanas de trabajo el lienzo es de SOLO LECTURA —
//! selección y checkbox de run, nada más—, y en ejecución las mutaciones
//! estructurales están deshabilitadas): tampoco se mueven nodos (el layout
//! es parte del documento de diseño) ni se reconectan/borran aristas.
//! Selección de paso y checkbox de run sí están siempre permitidos.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{cairo, DrawingArea};

use crate::theme::Palette;

use super::canvas_model::{
    has_dependency, CanvasData, CanvasModel, ConnectionSide, Edge, EdgeKind, Node, ViewTransform, CHECKBOX_SIZE,
};

// --- Colores del §6 --------------------------------------------------------

fn status_color(status: &opm_store::StepStatus, palette: Palette) -> &'static str {
    use opm_store::StepStatus;
    match status {
        StepStatus::Pending | StepStatus::Scheduled | StepStatus::Cancelled => palette.muted,
        StepStatus::Running => palette.color("warning").unwrap(),
        StepStatus::Ok => palette.color("success").unwrap(),
        StepStatus::Error => palette.color("danger").unwrap(),
    }
}

/// Etiqueta de la pastilla (minúsculas, paridad con el footer del SPA).
fn status_label(status: &opm_store::StepStatus) -> &'static str {
    use opm_store::StepStatus;
    match status {
        StepStatus::Pending => "pending",
        StepStatus::Scheduled => "scheduled",
        StepStatus::Running => "running",
        StepStatus::Ok => "ok",
        StepStatus::Error => "error",
        StepStatus::Cancelled => "cancelled",
    }
}

// --- Callbacks -------------------------------------------------------------

/// Acciones del lienzo hacia el integrador (`main.rs`).
pub struct CanvasCallbacks {
    /// Clic sobre una tarjeta: paso seleccionado (panel derecho).
    pub on_select: Rc<dyn Fn(String)>,
    /// Checkbox de la tarjeta: alterna el flag «selected» del store.
    pub on_toggle: Rc<dyn Fn(String)>,
    /// Arrastre nodo→nodo: `on_connect(source, target)` añade `source` a
    /// `target.dependencies` (unión vía `set_dependencies`).
    pub on_connect: Rc<dyn Fn(String, String)>,
    /// Delete sobre arista de dependencia: `on_disconnect(source, target)`.
    pub on_disconnect: Rc<dyn Fn(String, String)>,
    /// Reconexión del extremo de destino: remove+add — saca `old_source` de
    /// `old_target` y lo mete en `new_target`.
    pub on_reconnect: Rc<dyn Fn(String, String, String)>,
    /// Fin del drag de un nodo: persiste SOLO ese nodo (`store.set_layout`).
    pub on_move: Rc<dyn Fn(String, f64, f64)>,
    /// A completed click on empty canvas, transformed to world coordinates.
    pub on_place: Rc<dyn Fn(f64, f64)>,
    pub on_cancel_brush: Rc<dyn Fn()>,
    /// ¿Permitidas las mutaciones estructurales ahora mismo? (A12: false
    /// mientras corre la ejecución).
    pub edits_enabled: Rc<dyn Fn() -> bool>,
}

impl CanvasCallbacks {
    fn edits(&self) -> bool {
        (self.edits_enabled)()
    }
}

// --- Estado ----------------------------------------------------------------

/// Resolución del press: efectos de clic ya aplicados + interacción pendiente
/// para el drag (si llega).
enum Press {
    /// Checkbox → solo toggle, sin drag.
    Toggle,
    /// Mango de conexión → arrastre nodo→nodo.
    Connect(String, (f64, f64)),
    /// Extremo de destino de arista → reconectar.
    Reconnect(String, String, (f64, f64)),
    /// Tarjeta → seleccionar paso + posible movimiento.
    Node(String, (f64, f64)),
    /// Brush placement pending without a drag interaction.
    Select,
    /// Vacío → pan.
    Pan,
    /// Arista (ya seleccionada) sin interacción.
    Edge,
}

/// Interacción de drag en curso.
enum Interaction {
    None,
    Pan {
        start_pan: (f64, f64),
    },
    Node {
        id: String,
        offset: (f64, f64),
        start: (f64, f64),
        moved: bool,
    },
    Connect {
        source: String,
        anchor: (f64, f64),
        start: (f64, f64),
        cursor: (f64, f64),
    },
    Reconnect {
        source: String,
        target: String,
        anchor: (f64, f64),
        start: (f64, f64),
        cursor: (f64, f64),
    },
}

impl Interaction {
    fn from_press(press: Press, screen: (f64, f64), start_pan: (f64, f64)) -> Self {
        match press {
            Press::Toggle | Press::Edge | Press::Select => Interaction::None,
            Press::Connect(source, anchor) => Interaction::Connect {
                source,
                anchor,
                start: screen,
                cursor: screen,
            },
            Press::Reconnect(source, target, anchor) => Interaction::Reconnect {
                source,
                target,
                anchor,
                start: screen,
                cursor: screen,
            },
            Press::Node(id, offset) => Interaction::Node {
                id,
                offset,
                start: screen,
                moved: false,
            },
            Press::Pan => Interaction::Pan { start_pan },
        }
    }
}

struct CanvasState {
    /// Weak del `DrawingArea` para `queue_draw` sin retención (§11 A12).
    root: RefCell<Option<glib::WeakRef<DrawingArea>>>,
    /// Weak propio: el timer de animación lo lleva en su closure (§11 A12).
    this: RefCell<Option<WeakState>>,
    model: RefCell<CanvasModel>,
    /// Últimos datos aplicados (re-geometría durante un drag).
    data: RefCell<CanvasData>,
    transform: Cell<ViewTransform>,
    interaction: RefCell<Interaction>,
    press: Cell<Option<Press>>,
    brush_active: Cell<bool>,
    paint_origin: Cell<Option<(f64, f64)>>,
    /// Arista de dependencia seleccionada `(source, target)`.
    selected_edge: RefCell<Option<(String, String)>>,
    /// Actualización diferida mientras hay un drag en curso.
    deferred: RefCell<Option<CanvasData>>,
    /// Cursor conocido (motion) y tamaño del widget (para zoom sin cursor).
    pointer: Cell<Option<(f64, f64)>>,
    size: Cell<(f64, f64)>,
    /// Timer de animación del borde ámbar (§6).
    animation: RefCell<Option<glib::SourceId>>,
    boot: Instant,
    palette: Cell<Palette>,
    callbacks: CanvasCallbacks,
}

type WeakState = std::rc::Weak<CanvasState>;

impl CanvasState {
    fn new(callbacks: CanvasCallbacks, palette: Palette) -> Self {
        Self {
            root: RefCell::new(None),
            this: RefCell::new(None),
            model: RefCell::new(CanvasModel::default()),
            data: RefCell::new(CanvasData::default()),
            transform: Cell::new(ViewTransform::default()),
            interaction: RefCell::new(Interaction::None),
            press: Cell::new(None),
            brush_active: Cell::new(false),
            paint_origin: Cell::new(None),
            selected_edge: RefCell::new(None),
            deferred: RefCell::new(None),
            pointer: Cell::new(None),
            size: Cell::new((0.0, 0.0)),
            animation: RefCell::new(None),
            boot: Instant::now(),
            palette: Cell::new(palette),
            callbacks,
        }
    }

    fn queue(&self) {
        if let Some(root) = self
            .root
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        {
            root.queue_draw();
        }
    }

    fn cursor_or_center(&self) -> (f64, f64) {
        match self.pointer.get() {
            Some(pointer) => pointer,
            None => {
                let (width, height) = self.size.get();
                (width / 2.0, height / 2.0)
            }
        }
    }

    // --- Press (efectos de clic) ------------------------------------------

    /// Resuelve el press en `screen`: aplica los efectos de clic (selección,
    /// toggle, selección de arista) y deja la interacción pendiente para el
    /// drag. NO muta el grafo estructural (eso lo hace `main.rs` vía
    /// callbacks).
    fn press(&self, screen: (f64, f64)) {
        self.paint_origin.set(None);
        let transform = self.transform.get();
        let world = transform.to_world(screen);
        let model = self.model.borrow();

        // 1) Checkbox → toggle (sin drag; paridad SPA `stopPropagation`).
        if let Some(index) = model.node_at(world) {
            if model.hit_checkbox(index, world) {
                let id = model.nodes[index].id.clone();
                drop(model);
                (self.callbacks.on_toggle)(id);
                self.press.set(Some(Press::Toggle));
                return;
            }
        }

        // A selected dependency's target port reconnects it. Outside the
        // card, endpoint dragging keeps the existing reconnect affordance.
        if self.callbacks.edits() {
            if let Some((index, false)) = model.hit_edge_endpoint(screen, &transform) {
                let edge = &model.edges[index];
                let selected = self.selected_edge.borrow().as_ref()
                    .is_some_and(|(from, to)| from == &edge.source && to == &edge.target);
                if selected || model.node_at(world).is_none() {
                    let edge = edge.clone();
                    drop(model);
                    *self.selected_edge.borrow_mut() = Some((edge.source.clone(), edge.target.clone()));
                    self.press.set(Some(Press::Reconnect(edge.source, edge.target, edge.from)));
                    self.queue();
                    return;
                }
            }
            if let Some((index, side)) = model.hit_port(screen, &transform) {
                let node = &model.nodes[index];
                let source = node.id.clone();
                let anchor = node.anchor(side);
                drop(model);
                self.press.set(Some(Press::Connect(source, anchor)));
                return;
            }
        }

        // Layout is independent of structural editing: cards can always move,
        // including in Work or during execution. Only connections use edits().
        if let Some(index) = model.node_at(world) {
            let node = &model.nodes[index];
            let id = node.id.clone();
            let offset = (world.0 - node.position.0, world.1 - node.position.1);
            drop(model);
            *self.selected_edge.borrow_mut() = None;
            (self.callbacks.on_select)(id.clone());
            self.press.set(Some(Press::Node(id, offset)));
            return;
        }

        // 5) Arista de dependencia → seleccionar (Delete la borrará).
        if let Some(index) = model.hit_edge(screen, &transform) {
            let edge = model.edges[index].clone();
            drop(model);
            *self.selected_edge.borrow_mut() = Some((edge.source, edge.target));
            self.press.set(Some(Press::Edge));
            self.queue();
            return;
        }

        // 6) Vacío → pan + limpia la selección de arista.
        drop(model);
        *self.selected_edge.borrow_mut() = None;
        if self.brush_active.get() && self.callbacks.edits() {
            self.paint_origin.set(Some(screen));
            self.press.set(Some(Press::Select));
            return;
        }
        self.press.set(Some(Press::Pan));
        self.queue();
    }

    fn release(&self, screen: (f64, f64)) {
        let Some(origin) = self.paint_origin.take() else { return };
        if !self.brush_active.get() || !self.callbacks.edits()
            || (screen.0 - origin.0).hypot(screen.1 - origin.1) > 5.0 { return; }
        let world = self.transform.get().to_world(screen);
        let empty = self.model.borrow().node_at(world).is_none();
        if empty {
            (self.callbacks.on_place)(world.0, world.1);
        }
    }

    // --- Gestos de drag ----------------------------------------------------

    fn drag_begin(&self, screen: (f64, f64)) {
        let pending = self.press.take();
        let start_pan = (self.transform.get().pan_x, self.transform.get().pan_y);
        let mut interaction = match pending {
            Some(press) => Interaction::from_press(press, screen, start_pan),
            None => {
                // Drag sin press resuelto (orden de señales): resolver aquí.
                self.press(screen);
                match self.press.take() {
                    Some(press) => Interaction::from_press(press, screen, start_pan),
                    None => Interaction::None,
                }
            }
        };
        if let Interaction::Node { start, .. } = &mut interaction {
            // El press pudo calcular el offset con una pantalla previa:
            // se re-ancla al inicio real del drag.
            *start = screen;
        }
        *self.interaction.borrow_mut() = interaction;
    }

    /// `offset` es el desplazamiento respecto al inicio del drag (coordenadas
    /// de pantalla).
    fn drag_update(&self, offset: (f64, f64)) {
        {
            let mut interaction = self.interaction.borrow_mut();
            match &mut *interaction {
                Interaction::None => {}
                Interaction::Pan { start_pan } => {
                    let transform = self.transform.get();
                    self.transform.set(ViewTransform {
                        pan_x: start_pan.0 + offset.0,
                        pan_y: start_pan.1 + offset.1,
                        ..transform
                    });
                }
                Interaction::Node {
                    id,
                    offset: node_offset,
                    start,
                    moved,
                } => {
                    let screen = (start.0 + offset.0, start.1 + offset.1);
                    let world = self.transform.get().to_world(screen);
                    let target = (world.0 - node_offset.0, world.1 - node_offset.1);
                    let mut model = self.model.borrow_mut();
                    if let Some(index) = model.node_index(id) {
                        let node = &mut model.nodes[index];
                        if (node.position.0 - target.0).abs() > 0.5
                            || (node.position.1 - target.1).abs() > 0.5
                        {
                            *moved = true;
                            node.position = target;
                            let data = self.data.borrow().clone();
                            model.recompute_geometry(&data);
                        }
                    }
                }
                Interaction::Connect { start, cursor, .. }
                | Interaction::Reconnect { start, cursor, .. } => {
                    let screen = (start.0 + offset.0, start.1 + offset.1);
                    *cursor = self.transform.get().to_world(screen);
                }
            }
        }
        self.queue();
    }

    fn drag_end(&self) {
        let interaction =
            std::mem::replace(&mut *self.interaction.borrow_mut(), Interaction::None);
        // P2-6: la posición FINAL del nodo arrastrado se captura ANTES de
        // aplicar los datos diferidos (que reconstruyen el modelo desde el
        // layout persistido, todavía con las coordenadas viejas).
        let dragged = match &interaction {
            Interaction::Node { id, moved: true, .. } => {
                let model = self.model.borrow();
                model
                    .node_index(id)
                    .map(|index| (id.clone(), model.nodes[index].position))
            }
            _ => None,
        };
        // Las actualizaciones diferidas se aplican ANTES de los callbacks:
        // éstos refrescan con datos frescos del store y no deben quedar
        // tapadas por un dato del bombeo.
        let deferred = self.deferred.borrow_mut().take();
        if let Some(data) = deferred {
            self.apply(data);
        }
        match interaction {
            Interaction::None | Interaction::Pan { .. } => {}
            Interaction::Node { moved, .. } => {
                if moved {
                    if let Some((id, (x, y))) = dragged {
                        // P2-6: las coordenadas finales vuelven al modelo
                        // reconstruido (y su geometría de aristas) y se
                        // persisten SOLO ellas (§6 `store.set_layout`).
                        {
                            let data = self.data.borrow().clone();
                            let mut model = self.model.borrow_mut();
                            if let Some(index) = model.node_index(&id) {
                                model.nodes[index].position = (x, y);
                                model.recompute_geometry(&data);
                            }
                        }
                        (self.callbacks.on_move)(id, x, y);
                    }
                }
            }
            Interaction::Connect { source, cursor, .. } => {
                if self.callbacks.edits() {
                    let target = {
                        let model = self.model.borrow();
                        model
                            .node_at(cursor)
                            .or_else(|| model.hit_port(self.transform.get().to_screen(cursor), &self.transform.get()).map(|(index, _)| index))
                            .map(|index| model.nodes[index].id.clone())
                    };
                    if let Some(target) = target {
                        if target != source {
                            (self.callbacks.on_connect)(source, target);
                        }
                    }
                }
            }
            Interaction::Reconnect {
                source,
                target,
                cursor,
                ..
            } => {
                if self.callbacks.edits() {
                    let new_target = {
                        let model = self.model.borrow();
                        model
                            .node_at(cursor)
                            .or_else(|| model.hit_port(self.transform.get().to_screen(cursor), &self.transform.get()).map(|(index, _)| index))
                            .map(|index| model.nodes[index].id.clone())
                    };
                    if let Some(new_target) = new_target {
                        if new_target != target && new_target != source {
                            (self.callbacks.on_reconnect)(source, target, new_target);
                        }
                    }
                }
            }
        }
        self.queue();
    }

    /// Delete/Backspace: borra la arista de dependencia seleccionada (A9).
    fn delete_selected_edge(&self) {
        if !self.callbacks.edits() {
            return;
        }
        let selected = self.selected_edge.borrow_mut().take();
        if let Some((source, target)) = selected {
            (self.callbacks.on_disconnect)(source, target);
            self.queue();
        }
    }

    /// Zoom con ctrl+rueda anclado al cursor (o al centro).
    fn zoom(&self, dy: f64) {
        let factor = if dy < 0.0 { 1.1 } else { 1.0 / 1.1 };
        let mut transform = self.transform.get();
        transform.zoom_at(self.cursor_or_center(), factor);
        self.transform.set(transform);
        self.queue();
    }

    fn fit(&self) {
        let model = self.model.borrow();
        if model.nodes.is_empty() { return; }
        let mut min_x = model.nodes.iter().map(|n| n.position.0).fold(f64::INFINITY, f64::min);
        let mut min_y = model.nodes.iter().map(|n| n.position.1).fold(f64::INFINITY, f64::min);
        let mut max_x = model.nodes.iter().map(|n| n.rect().0 + n.rect().2).fold(f64::NEG_INFINITY, f64::max);
        let mut max_y = model.nodes.iter().map(|n| n.rect().1 + n.rect().3).fold(f64::NEG_INFINITY, f64::max);
        for edge in &model.edges {
            for (x, y) in edge.sample(32) {
                min_x = min_x.min(x); min_y = min_y.min(y);
                max_x = max_x.max(x); max_y = max_y.max(y);
            }
        }
        let (width, height) = self.size.get();
        let zoom = ((width - 48.0) / (max_x - min_x)).min((height - 80.0) / (max_y - min_y))
            .clamp(ViewTransform::MIN_ZOOM, 1.0);
        self.transform.set(ViewTransform {
            zoom, pan_x: (width - (max_x - min_x) * zoom) / 2.0 - min_x * zoom,
            pan_y: (height - 40.0 - (max_y - min_y) * zoom) / 2.0 - min_y * zoom,
        });
        self.queue();
    }

    fn keyboard(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        if modifiers.contains(gdk::ModifierType::ALT_MASK) {
            let delta = match key {
                gdk::Key::Left => (40.0, 0.0), gdk::Key::Right => (-40.0, 0.0),
                gdk::Key::Up => (0.0, 40.0), gdk::Key::Down => (0.0, -40.0),
                _ => return false,
            };
            let mut transform = self.transform.get();
            transform.pan_x += delta.0; transform.pan_y += delta.1;
            self.transform.set(transform); self.queue(); return true;
        }
        if !modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SUPER_MASK) {
            match key {
                gdk::Key::Home => { self.fit(); return true; }
                gdk::Key::plus | gdk::Key::equal => { self.zoom(-1.0); return true; }
                gdk::Key::minus => { self.zoom(1.0); return true; }
                gdk::Key::Left | gdk::Key::Right => {
                    let id = {
                        let model = self.model.borrow();
                        if model.nodes.is_empty() { return false; }
                        let current = model.nodes.iter().position(|node| node.active);
                        let next = match current {
                            None => 0,
                            Some(i) if key == gdk::Key::Left => i.saturating_sub(1),
                            Some(i) => (i + 1).min(model.nodes.len() - 1),
                        };
                        let node = &model.nodes[next];
                        // Keep keyboard selection visible without changing stored layout.
                        let mut transform = self.transform.get();
                        let (x, y) = transform.to_screen(node.position);
                        let (width, height) = self.size.get();
                        let (_, _, w, h) = node.rect();
                        if x < 12.0 { transform.pan_x += 12.0 - x; }
                        else if x + w * transform.zoom > width - 12.0 {
                            transform.pan_x += width - 12.0 - x - w * transform.zoom;
                        }
                        if y < 12.0 { transform.pan_y += 12.0 - y; }
                        else if y + h * transform.zoom > height - 12.0 {
                            transform.pan_y += height - 12.0 - y - h * transform.zoom;
                        }
                        self.transform.set(transform);
                        node.id.clone()
                    };
                    self.selected_edge.borrow_mut().take();
                    (self.callbacks.on_select)(id); self.queue(); return true;
                }
                gdk::Key::space => {
                    let id = self.model.borrow().nodes.iter().find(|node| node.active).map(|n| n.id.clone());
                    if let Some(id) = id { (self.callbacks.on_toggle)(id); return true; }
                }
                _ => {}
            }
        }
        false
    }

    // --- Actualización de datos -------------------------------------------

    /// Aplica los datos del store; si hay un drag en curso se DIFIERE (el
    /// drag es la autoridad de posición hasta soltar).
    fn update(&self, data: CanvasData) {
        let dragging = !matches!(&*self.interaction.borrow(), Interaction::None);
        if dragging {
            *self.deferred.borrow_mut() = Some(data);
            return;
        }
        self.apply(data);
    }

    fn apply(&self, data: CanvasData) {
        let model = CanvasModel::build(&data);
        // La selección de arista sobrevive al refresco solo si la arista
        // sigue existiendo.
        let selected_edge = self.selected_edge.borrow().clone();
        if let Some((source, target)) = selected_edge {
            if !has_dependency(&model, &source, &target) {
                self.selected_edge.borrow_mut().take();
            }
        }
        if let Some(root) = self.root.borrow().as_ref().and_then(glib::WeakRef::upgrade) {
            let name = model.nodes.iter().find(|node| node.active)
                .map(|node| format!("Pipeline graph. {}, {}, {}", node.name, node.id, status_label(&node.status)))
                .unwrap_or_else(|| "Pipeline graph".into());
            root.update_property(&[gtk::accessible::Property::Label(&name)]);
            root.set_tooltip_text(Some(if self.callbacks.edits() {
                "Drag cards to move · Drag ports to connect · Ctrl+scroll to zoom · Home to fit · Left/Right to select"
            } else {
                "Read-only pipeline · Drag background to pan · Ctrl+scroll to zoom · Home to fit · Left/Right to select · Space toggles run selection"
            }));
        }
        *self.data.borrow_mut() = data;
        *self.model.borrow_mut() = model;
        self.sync_animation();
        self.queue();
    }

    /// Timer de animación del borde ámbar: solo mientras hay `running`; el
    /// closure lleva el `Weak` del estado (§11 A12: sin retención, se
    /// auto-detiene al morir el lienzo).
    fn sync_animation(&self) {
        let any_running = self
            .model
            .borrow()
            .nodes
            .iter()
            .any(|node| node.status == opm_store::StepStatus::Running);
        let mut animation = self.animation.borrow_mut();
        if any_running {
            if animation.is_none() {
                let Some(weak) = self.this.borrow().clone() else {
                    return;
                };
                let source = glib::timeout_add_local(Duration::from_millis(90), move || {
                    let Some(state) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    state.queue();
                    glib::ControlFlow::Continue
                });
                *animation = Some(source);
            }
        } else if let Some(source) = animation.take() {
            source.remove();
        }
    }

    // --- Dibujo ------------------------------------------------------------

    fn draw(&self, cr: &cairo::Context, width: f64, height: f64) {
        self.size.set((width, height));
        let palette = self.palette.get();
        // Suelo del canvas dentro del panel (§6 «borde + cabecera»): relleno
        // plano apenas tintado —distinto del fondo de la ventana que asoma
        // en el inset del `opm-panel-body`— con la rejilla de puntos encima.
        let (r, g, b) = super::canvas_model::hex_rgb(palette.code_bg);
        cr.set_source_rgb(r, g, b);
        let _ = cr.paint();

        let model = self.model.borrow();

        if let Some(message) = &model.empty {
            let _ = cr.select_font_face(
                "Sans",
                cairo::FontSlant::Normal,
                cairo::FontWeight::Normal,
            );
            cr.set_font_size(14.0);
            let (tx, ty) = match cr.text_extents(message) {
                Ok(extents) => (
                    ((width - extents.width()) / 2.0).max(8.0),
                    (height - extents.height()) / 2.0,
                ),
                Err(_) => (8.0, height / 2.0),
            };
            let (r, g, b) = super::canvas_model::hex_rgb(palette.muted);
            let _ = cr.set_source_rgb(r, g, b);
            cr.move_to(tx, ty);
            let _ = cr.show_text(message);
            return;
        }

        // Rejilla sutil en coordenadas de mundo: se calcula sobre la parte
        // visible y se DIBUJA a través de la MISMA transformada que nodos y
        // aristas (A9: un solo translate/zoom para todo el render — la rejilla
        // tiene que desplazarse y escalar con el resto); se omite si serían
        // demasiados puntos (zoom muy lejano).
        let transform = self.transform.get();
        let top_left = transform.to_world((0.0, 0.0));
        let bottom_right = transform.to_world((width, height));
        const GRID: f64 = 28.0;
        let columns = ((bottom_right.0 - top_left.0) / GRID).ceil() as i64;
        let rows = ((bottom_right.1 - top_left.1) / GRID).ceil() as i64;
        let grid = (columns > 0 && rows > 0 && columns * rows <= 4_000).then(|| {
            let origin_x = (top_left.0 / GRID).floor() * GRID;
            let origin_y = (top_left.1 / GRID).floor() * GRID;
            (columns, rows, origin_x, origin_y)
        });

        let _ = cr.save();
        cr.translate(transform.pan_x, transform.pan_y);
        cr.scale(transform.zoom, transform.zoom);

        if let Some((columns, rows, origin_x, origin_y)) = grid {
            let (r, g, b) = super::canvas_model::hex_rgb(palette.rule);
            let _ = cr.set_source_rgba(r, g, b, 0.55);
            let mut column = 0;
            while column <= columns {
                let mut row = 0;
                while row <= rows {
                    let x = origin_x + column as f64 * GRID;
                    let y = origin_y + row as f64 * GRID;
                    cr.rectangle(x, y, 1.6, 1.6);
                    row += 1;
                }
                column += 1;
            }
            let _ = cr.fill();
        }

        let selected = self.selected_edge.borrow().clone();
        let font_family = if gtk::is_initialized() {
            gtk::Settings::default().and_then(|s| s.gtk_font_name())
                .and_then(|font| gtk::pango::FontDescription::from_string(&font).family())
                .map(|family| family.to_string()).unwrap_or_else(|| "Sans".into())
        } else { "Sans".into() };
        for edge in &model.edges {
            draw_edge(cr, edge, &selected, self.boot, palette);
        }
        draw_pending_link(cr, &self.interaction.borrow(), palette);
        for node in &model.nodes {
            draw_node(cr, node, palette, &font_family);
        }

        let _ = cr.restore();
    }
}

/// Arista según su clase (A9) + flecha + ámbar animado del destino (§6).
fn draw_edge(cr: &cairo::Context, edge: &Edge, selected: &Option<(String, String)>, boot: Instant, palette: Palette) {
    let is_selected = matches!(
        selected,
        Some((source, target)) if source == &edge.source && target == &edge.target
    );
    let base_color = if edge.running {
        palette.color("warning").unwrap()
    } else {
        match edge.kind {
            EdgeKind::Order => palette.muted,
            EdgeKind::Output => palette.output_link,
            EdgeKind::Dependency => {
                if is_selected {
                    palette.color("accent").unwrap()
                } else {
                    palette.dependency_link
                }
            }
        }
    };
    let (r, g, b) = super::canvas_model::hex_rgb(base_color);
    let _ = cr.set_source_rgb(r, g, b);
    cr.set_line_width(match (edge.running, is_selected) {
        (true, _) => {
            let animate = gtk::is_initialized() && gtk::Settings::default()
                .is_some_and(|settings| settings.is_gtk_enable_animations());
            2.8 + if animate { (boot.elapsed().as_secs_f64() * 3.0).sin() * 0.3 } else { 0.0 }
        },
        (_, true) => 3.4,
        _ => match edge.kind {
            EdgeKind::Order => 1.3,
            EdgeKind::Output => 2.0,
            EdgeKind::Dependency => 2.4,
        },
    });
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.move_to(edge.from.0, edge.from.1);
    cr.curve_to(
        edge.control1.0,
        edge.control1.1,
        edge.control2.0,
        edge.control2.1,
        edge.to.0,
        edge.to.1,
    );
    // All links are continuous; color, weight and arrow shape distinguish
    // their roles. Running links gently pulse in width when motion is enabled.
    cr.set_dash(&[], 0.0);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
    arrow_head(cr, edge.to, edge.control2, base_color, edge.kind, palette);
}

/// Línea temporal de conexión/reconexión (nodo→nodo en curso).
fn draw_pending_link(cr: &cairo::Context, interaction: &Interaction, palette: Palette) {
    let (from, to) = match interaction {
        Interaction::Connect { anchor, cursor, .. }
        | Interaction::Reconnect { anchor, cursor, .. } => (*anchor, *cursor),
        _ => return,
    };
    let (r, g, b) = super::canvas_model::hex_rgb(palette.dependency_link);
    let _ = cr.set_source_rgb(r, g, b);
    cr.set_line_width(2.4);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.move_to(from.0, from.1);
    cr.line_to(to.0, to.1);
    cr.set_dash(&[], 0.0);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
}

/// Tarjeta: fondo redondeado, checkbox, id, pastilla de estado y nombre.
fn draw_node(cr: &cairo::Context, node: &Node, palette: Palette, font_family: &str) {
    let (x, y, width, height) = node.rect();

    let accent = palette.color("accent").unwrap();
    let (r, g, b) = super::canvas_model::hex_rgb(palette.bg);
    rounded_rect(cr, x, y, width, height, 8.0);
    cr.set_source_rgb(r, g, b);
    let _ = cr.fill_preserve();
    let (r, g, b) = super::canvas_model::hex_rgb(if node.active { accent } else { palette.rule });
    cr.set_source_rgb(r, g, b);
    cr.set_line_width(if node.active { 2.4 } else { 1.2 });
    let _ = cr.stroke();

    let (cx, cy, cs) = (x + super::canvas_model::CHECKBOX_OFFSET.0,
        y + super::canvas_model::CHECKBOX_OFFSET.1, CHECKBOX_SIZE);
    rounded_rect(cr, cx, cy, cs, cs, 3.0);
    if node.checked {
        // Light accent is used for the filled checkbox in both themes so
        // the white check retains contrast; selection outline uses theme accent.
        let (r, g, b) = super::canvas_model::hex_rgb(Palette::LIGHT.color("accent").unwrap());
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.move_to(cx + 3.0, cy + 7.5);
        cr.line_to(cx + 6.0, cy + 10.5);
        cr.line_to(cx + 11.0, cy + 4.0);
        let _ = cr.stroke();
    } else {
        let (r, g, b) = super::canvas_model::hex_rgb(palette.muted);
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }
    // Title is the primary identifier; the full stable ID has its own row.
    cr.select_font_face(font_family, cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(13.5);
    let text = fit_text(cr, &node.name, width - 56.0);
    let (r, g, b) = super::canvas_model::hex_rgb(palette.fg);
    cr.set_source_rgb(r, g, b);
    cr.move_to(x + 40.0, y + 27.0);
    let _ = cr.show_text(&text);
    cr.select_font_face("Monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);
    let text = fit_text(cr, &node.id, width - 32.0);
    let (r, g, b) = super::canvas_model::hex_rgb(palette.muted);
    cr.set_source_rgb(r, g, b);
    cr.move_to(x + 16.0, y + 49.0);
    let _ = cr.show_text(&text);

    // Text and a dot convey status without low-contrast white-on-gray pills.
    let (r, g, b) = super::canvas_model::hex_rgb(status_color(&node.status, palette));
    cr.set_source_rgb(r, g, b);
    cr.arc(x + 20.0, y + 76.0, 3.5, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    cr.select_font_face(font_family, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.5);
    cr.move_to(x + 30.0, y + 80.0);
    let _ = cr.show_text(status_label(&node.status));

    // Connection targets share the unchanged model anchors/hit regions.
    for side in ConnectionSide::ALL {
        let anchor = node.anchor(side);
        cr.new_path();
        cr.arc(anchor.0, anchor.1, 4.0, 0.0, std::f64::consts::TAU);
        let (r, g, b) = super::canvas_model::hex_rgb(palette.bg);
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill_preserve();
        let (r, g, b) = super::canvas_model::hex_rgb(if node.active { accent } else { palette.muted });
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }
}

/// Flecha en el extremo `tip` apuntando desde `from`.
fn arrow_head(cr: &cairo::Context, tip: (f64, f64), from: (f64, f64), color: &str, kind: EdgeKind, palette: Palette) {
    let angle = (tip.1 - from.1).atan2(tip.0 - from.0);
    // Leave room for the circular connection port at the card boundary.
    let tip = (tip.0 - angle.cos() * 5.0, tip.1 - angle.sin() * 5.0);
    let spread = 0.52;
    let length = if kind == EdgeKind::Order { 7.0 } else { 10.0 };
    let left = angle + std::f64::consts::PI - spread;
    let right = angle + std::f64::consts::PI + spread;
    let (r, g, b) = super::canvas_model::hex_rgb(color);
    cr.set_source_rgb(r, g, b);
    cr.new_path();
    cr.move_to(tip.0 + left.cos() * length, tip.1 + left.sin() * length);
    cr.line_to(tip.0, tip.1);
    cr.line_to(tip.0 + right.cos() * length, tip.1 + right.sin() * length);
    if kind == EdgeKind::Order {
        cr.set_line_width(1.3);
        let _ = cr.stroke();
    } else {
        cr.close_path();
        if kind == EdgeKind::Output {
            let (r, g, b) = super::canvas_model::hex_rgb(palette.code_bg);
            cr.set_source_rgb(r, g, b);
            let _ = cr.fill_preserve();
            let (r, g, b) = super::canvas_model::hex_rgb(color);
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(1.8);
            let _ = cr.stroke();
        } else {
            let _ = cr.fill();
        }
    }
}

/// Rectángulo redondeado manual (cairo-rs no lo trae).
fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, radius: f64) {
    let radius = radius.min(w / 2.0).min(h / 2.0);
    let right = x + w;
    let bottom = y + h;
    cr.new_sub_path();
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        1.5 * std::f64::consts::PI,
    );
    cr.arc(
        right - radius,
        y + radius,
        radius,
        1.5 * std::f64::consts::PI,
        2.0 * std::f64::consts::PI,
    );
    cr.arc(right - radius, bottom - radius, radius, 0.0, 0.5 * std::f64::consts::PI);
    cr.arc(
        x + radius,
        bottom - radius,
        radius,
        0.5 * std::f64::consts::PI,
        std::f64::consts::PI,
    );
    cr.close_path();
}

/// Trunca `text` con `…` hasta que quepa en `max_width` (medido con la
/// fuente activa).
fn fit_text(cr: &cairo::Context, text: &str, max_width: f64) -> String {
    let width_of = |value: &str| {
        cr.text_extents(value)
            .map(|extents| extents.width())
            .unwrap_or(0.0)
    };
    if width_of(text) <= max_width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if width_of(&candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

// --- Widget público --------------------------------------------------------

/// Lienzo del pipeline (página permanente del stack central, §11 A11).
pub struct PipelineCanvas {
    root: DrawingArea,
    state: Rc<CanvasState>,
}

impl PipelineCanvas {
    pub fn new(palette: Palette, callbacks: CanvasCallbacks) -> Rc<Self> {
        let state = Rc::new(CanvasState::new(callbacks, palette));
        *state.this.borrow_mut() = Some(Rc::downgrade(&state));

        let area = DrawingArea::new();
        if let Some(settings) = gtk::Settings::default() {
            if let Some(font) = settings.gtk_font_name() {
                let points = gtk::pango::FontDescription::from_string(&font).size() as f64
                    / gtk::pango::SCALE as f64;
                state.transform.set(ViewTransform { zoom: (points / 11.0).clamp(1.0, 2.5), ..ViewTransform::default() });
            }
        }
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_focusable(true);
        area.set_accessible_role(gtk::AccessibleRole::Group);
        area.set_size_request(240, 160);
        area.update_property(&[
            gtk::accessible::Property::Label("Pipeline graph"),
            gtk::accessible::Property::Description("Left and Right select a step; Space toggles its run checkbox. Alt+arrows pan; plus and minus zoom. Drag cards to move, ports to connect. Delete removes the selected dependency."),
        ]);
        area.set_tooltip_text(Some("Drag to pan · Ctrl+scroll to zoom · Left/Right to select · Space to toggle run selection"));
        area.add_css_class("opm-canvas");

        area.set_draw_func({
            let state = Rc::clone(&state);
            move |_, cr, width, height| state.draw(cr, width as f64, height as f64)
        });

        // Weak del widget para `queue_draw` sin ciclos (§11 A12).
        *state.root.borrow_mut() = Some(area.downgrade());

        // Clic: efectos de selección/checkbox/arista.
        area.add_controller({
            let state = Rc::clone(&state);
            let click = gtk::GestureClick::new();
            click.connect_released({
                let state = Rc::clone(&state);
                move |gesture, _, x, y| {
                    if gesture.current_button() == gdk::BUTTON_PRIMARY { state.release((x, y)); }
                }
            });
            click.connect_pressed(move |gesture, _n_press, x, y| {
                if gesture.current_button() != gdk::BUTTON_PRIMARY {
                    return;
                }
                if let Some(widget) = gesture.widget() { widget.grab_focus(); }
                state.press((x, y));
            });
            click
        });

        // Drag: pan / mover nodo / conectar / reconectar.
        area.add_controller({
            let drag = gtk::GestureDrag::new();
            drag.set_button(gdk::BUTTON_PRIMARY);
            drag.connect_drag_begin({
                let state = Rc::clone(&state);
                move |_, x, y| state.drag_begin((x, y))
            });
            drag.connect_drag_update({
                let state = Rc::clone(&state);
                move |_, offset_x, offset_y| state.drag_update((offset_x, offset_y))
            });
            drag.connect_drag_end({
                let state = Rc::clone(&state);
                move |_, _offset_x, _offset_y| state.drag_end()
            });
            drag
        });

        // Cursor conocido (zoom anclado + línea de conexión).
        area.add_controller({
            let state = Rc::clone(&state);
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, x, y| state.pointer.set(Some((x, y))));
            motion
        });

        // Ctrl+rueda: zoom anclado al cursor (A9).
        area.add_controller({
            let state = Rc::clone(&state);
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL
                    | gtk::EventControllerScrollFlags::HORIZONTAL,
            );
            scroll.connect_scroll(move |controller, _dx, dy| {
                if !controller
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK)
                {
                    return glib::Propagation::Proceed;
                }
                if dy.abs() < f64::EPSILON {
                    return glib::Propagation::Proceed;
                }
                state.zoom(dy);
                glib::Propagation::Stop
            });
            scroll
        });

        // Delete/Backspace sobre arista seleccionada (A9).
        area.add_controller({
            let state = Rc::clone(&state);
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _code, modifiers| {
                if state.keyboard(key, modifiers) { return glib::Propagation::Stop; }
                if key == gdk::Key::Escape && state.brush_active.get() {
                    (state.callbacks.on_cancel_brush)();
                    return glib::Propagation::Stop;
                }
                if matches!(key, gdk::Key::Delete | gdk::Key::BackSpace) {
                    let had_selection = state.selected_edge.borrow().is_some();
                    state.delete_selected_edge();
                    if had_selection {
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
            keys
        });

        Rc::new(Self { root: area, state })
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn set_palette(&self, palette: Palette) {
        self.state.palette.set(palette);
        self.root.queue_draw();
    }

    /// Native, keyboard-focusable controls above the Cairo surface.
    pub fn viewport_controls(&self) -> gtk::Box {
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls.add_css_class("linked");
        controls.add_css_class("opm-viewport-controls");
        for (icon, label, key) in [
            (Some("zoom-out-symbolic"), "Zoom out (−)", gdk::Key::minus),
            (None, "Fit pipeline (Home)", gdk::Key::Home),
            (Some("zoom-in-symbolic"), "Zoom in (+)", gdk::Key::plus),
        ] {
            let button = if let Some(icon) = icon { gtk::Button::from_icon_name(icon) }
                else { gtk::Button::with_label("Fit") };
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
            let state = self.state.clone();
            button.connect_clicked(move |_| { state.keyboard(key, gdk::ModifierType::empty()); });
            controls.append(&button);
        }
        let help = gtk::MenuButton::new();
        help.set_icon_name("help-about-symbolic");
        help.set_tooltip_text(Some("Canvas controls and link types"));
        help.update_property(&[gtk::accessible::Property::Label("Canvas controls and link types")]);
        let popover = gtk::Popover::new();
        let text = gtk::Label::new(Some("Drag background to pan\nCtrl+scroll or + / − to zoom\nHome to fit the pipeline\nLeft / Right to select a step\nSpace to toggle its run checkbox\n\nLinks\nGray thin / chevron: step order\nTeal / open arrow: output flow\nPurple / filled arrow: dependency\n\nDrag cards to move them in Design or Work. Work saves positions in its own folder. In Design, drag any of the four ports to connect steps. Links use top/bottom ports for vertical layouts and side ports for horizontal layouts. Select a dependency and press Delete to remove it, or drag its destination port to reconnect it. Escape returns from the tool brush to selection."));
        text.set_wrap(true);
        text.set_max_width_chars(38);
        text.set_xalign(0.0);
        text.set_margin_top(12);
        text.set_margin_bottom(12);
        text.set_margin_start(12);
        text.set_margin_end(12);
        popover.set_child(Some(&text));
        help.set_popover(Some(&popover));
        controls.append(&help);
        controls
    }

    pub fn set_brush_active(&self, active: bool) {
        self.state.brush_active.set(active);
        self.state.paint_origin.set(None);
        self.root.set_cursor_from_name(if active { Some("crosshair") } else { None });
    }

    /// Refresca el grafo desde los datos del store. Con un drag en curso la
    /// actualización se difiere hasta soltar (el drag manda en posiciones).
    pub fn update(&self, data: CanvasData) {
        self.state.update(data);
    }

    /// Estado vacío visible (tests).
    #[cfg(test)]
    pub fn empty_message(&self) -> Option<String> {
        self.state.model.borrow().empty.clone()
    }

    /// Zoom/pan actuales (tests).
    #[cfg(test)]
    pub fn transform(&self) -> ViewTransform {
        self.state.transform.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use opm_pipeline::PipelineStep;
    use opm_store::Position;

    #[test]
    fn brush_places_once_in_world_coordinates_and_respects_drag_and_readonly_gates() {
        let placed = Rc::new(RefCell::new(Vec::new()));
        let edits = Rc::new(Cell::new(true));
        let state = CanvasState::new(CanvasCallbacks {
            on_select: Rc::new(|_| {}), on_toggle: Rc::new(|_| {}),
            on_connect: Rc::new(|_, _| {}), on_disconnect: Rc::new(|_, _| {}),
            on_reconnect: Rc::new(|_, _, _| {}), on_move: Rc::new(|_, _, _| {}),
            on_place: { let placed = placed.clone(); Rc::new(move |x, y| placed.borrow_mut().push((x, y))) },
            on_cancel_brush: Rc::new(|| {}),
            edits_enabled: { let edits = edits.clone(); Rc::new(move || edits.get()) },
        }, Palette::LIGHT);
        state.brush_active.set(true);
        state.transform.set(ViewTransform { zoom: 2.0, pan_x: 20.0, pan_y: -10.0 });
        state.press((100.0, 50.0));
        state.drag_begin((100.0, 50.0));
        state.release((100.0, 50.0));
        state.release((100.0, 50.0));
        assert_eq!(*placed.borrow(), vec![(40.0, 30.0)]);
        state.press((100.0, 50.0)); state.release((150.0, 50.0));
        assert_eq!(placed.borrow().len(), 1);
        state.press((100.0, 50.0)); edits.set(false); state.release((100.0, 50.0));
        assert_eq!(placed.borrow().len(), 1);
        edits.set(true); state.brush_active.set(false);
        state.press((100.0, 50.0)); state.release((100.0, 50.0));
        assert_eq!(placed.borrow().len(), 1);
    }

    /// Datos de un proyecto con un único paso en `position`.
    fn data_with(id: &str, position: (f64, f64)) -> CanvasData {
        let step = PipelineStep {
            id: id.to_owned(),
            name: format!("Step {id}"),
            description: String::new(),
            command: String::new(),
            environment: String::new(),
            working_directory: ".".to_owned(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            parameters: Vec::new(),
            dependencies: Vec::new(),
        };
        let mut layout = HashMap::new();
        layout.insert(
            id.to_owned(),
            Position {
                x: position.0,
                y: position.1,
            },
        );
        CanvasData {
            context: crate::CanvasContext::Work,
            steps: vec![step],
            states: HashMap::new(),
            layout,
            selected: None,
        }
    }

    fn state_recording_moves(
        moves: &Rc<RefCell<Vec<(String, f64, f64)>>>,
    ) -> CanvasState {
        let recorded = Rc::clone(moves);
        CanvasState::new(
            CanvasCallbacks {
                on_select: Rc::new(|_: String| {}),
                on_toggle: Rc::new(|_: String| {}),
                on_connect: Rc::new(|_: String, _: String| {}),
                on_disconnect: Rc::new(|_: String, _: String| {}),
                on_reconnect: Rc::new(|_: String, _: String, _: String| {}),
                on_move: Rc::new(move |id, x, y| {
                    recorded.borrow_mut().push((id, x, y))
                }),
                on_place: Rc::new(|_, _| {}),
                on_cancel_brush: Rc::new(|| {}),
                edits_enabled: Rc::new(|| true),
            },
            Palette::LIGHT,
        )
    }

    #[test]
    fn keyboard_navigation_and_fit_do_not_mutate_stored_layout() {
        let moves = Rc::new(RefCell::new(Vec::new()));
        let mut state = state_recording_moves(&moves);
        let selected = Rc::new(RefCell::new(Vec::new()));
        let toggled = Rc::new(RefCell::new(Vec::new()));
        state.callbacks.on_select = { let selected = selected.clone(); Rc::new(move |id| selected.borrow_mut().push(id)) };
        state.callbacks.on_toggle = { let toggled = toggled.clone(); Rc::new(move |id| toggled.borrow_mut().push(id)) };
        // Read-only work graphs still support selection and run checkboxes.
        state.callbacks.edits_enabled = Rc::new(|| false);
        let mut data = data_with("s1", (900.0, 800.0));
        data.selected = Some("s1".into());
        state.size.set((500.0, 400.0));
        state.update(data);
        assert!(!state.keyboard(gdk::Key::Left, gdk::ModifierType::CONTROL_MASK));
        assert!(selected.borrow().is_empty());
        assert!(state.keyboard(gdk::Key::Left, gdk::ModifierType::empty()));
        assert_eq!(*selected.borrow(), vec!["s1"]);
        assert!(state.keyboard(gdk::Key::space, gdk::ModifierType::empty()));
        assert_eq!(*toggled.borrow(), vec!["s1"]);
        assert!(state.keyboard(gdk::Key::Home, gdk::ModifierType::empty()));
        let position = state.transform.get().to_screen((900.0, 800.0));
        assert!(position.0 >= 0.0 && position.1 >= 0.0);
        assert_eq!(state.model.borrow().nodes[0].position, (900.0, 800.0));
        assert!(moves.borrow().is_empty());
    }

    #[test]
    fn refresh_drops_a_removed_selected_dependency_without_borrow_conflict() {
        let moves = Rc::new(RefCell::new(Vec::new()));
        let state = state_recording_moves(&moves);
        *state.selected_edge.borrow_mut() = Some(("removed".into(), "s1".into()));
        state.update(data_with("s1", (80.0, 120.0)));
        assert!(state.selected_edge.borrow().is_none());
    }

    #[test]
    fn node_layout_can_move_when_structural_edits_are_disabled() {
        let moves = Rc::new(RefCell::new(Vec::new()));
        let mut state = state_recording_moves(&moves);
        state.callbacks.edits_enabled = Rc::new(|| false);
        state.update(data_with("s1", (80.0, 120.0)));
        let transform = ViewTransform { zoom: 0.5, pan_x: 17.0, pan_y: -13.0 };
        state.transform.set(transform);
        state.drag_begin(transform.to_screen((180.0, 175.0)));
        state.drag_update((50.0, 25.0));
        state.drag_end();
        assert_eq!(*moves.borrow(), vec![("s1".into(), 180.0, 170.0)]);
    }

    #[test]
    fn dependency_drag_and_delete_keep_the_edit_permission_gate() {
        let moves = Rc::new(RefCell::new(Vec::new()));
        let mut state = state_recording_moves(&moves);
        let connections = Rc::new(RefCell::new(Vec::new()));
        let removals = Rc::new(RefCell::new(Vec::new()));
        let editable = Rc::new(Cell::new(true));
        state.callbacks.on_connect = {
            let connections = connections.clone();
            Rc::new(move |from, to| connections.borrow_mut().push((from, to)))
        };
        state.callbacks.on_disconnect = {
            let removals = removals.clone();
            Rc::new(move |from, to| removals.borrow_mut().push((from, to)))
        };
        state.callbacks.edits_enabled = {
            let editable = editable.clone();
            Rc::new(move || editable.get())
        };
        let mut data = data_with("source", (80.0, 120.0));
        let target = data_with("target", (450.0, 120.0));
        data.steps.extend(target.steps);
        data.layout.extend(target.layout);
        state.update(data);
        state.drag_begin((340.0, 168.0));
        state.drag_update((160.0, 0.0));
        state.drag_end();
        assert_eq!(*connections.borrow(), vec![("source".to_owned(), "target".to_owned())]);
        *state.selected_edge.borrow_mut() = Some(("source".into(), "target".into()));
        editable.set(false);
        state.delete_selected_edge();
        assert!(removals.borrow().is_empty());
        assert!(state.selected_edge.borrow().is_some());
        editable.set(true);
        state.delete_selected_edge();
        assert_eq!(*removals.borrow(), *connections.borrow());
        assert!(moves.borrow().is_empty());
    }

    #[test]
    fn drag_starts_at_each_port_and_accepts_a_drop_outside_the_target_border() {
        let moves = Rc::new(RefCell::new(Vec::new()));
        let mut state = state_recording_moves(&moves);
        let connections = Rc::new(RefCell::new(Vec::new()));
        state.callbacks.on_connect = {
            let connections = connections.clone();
            Rc::new(move |from, to| connections.borrow_mut().push((from, to)))
        };
        let mut data = data_with("source", (80.0, 120.0));
        let target = data_with("target", (450.0, 350.0));
        data.steps.extend(target.steps);
        data.layout.extend(target.layout);
        state.update(data);
        let transform = ViewTransform { zoom: 0.7, pan_x: -25.0, pan_y: 30.0 };
        state.transform.set(transform);
        for side in ConnectionSide::ALL {
            let anchor = state.model.borrow().nodes[0].anchor(side);
            let from = transform.to_screen(anchor);
            let to = transform.to_screen((580.0, 345.0));
            state.drag_begin(from);
            assert!(matches!(*state.interaction.borrow(), Interaction::Connect { anchor: actual, .. } if actual == anchor));
            state.drag_update((to.0 - from.0, to.1 - from.1));
            state.drag_end();
        }
        assert_eq!(*connections.borrow(), vec![("source".to_owned(), "target".to_owned()); 4]);
        assert!(moves.borrow().is_empty());
    }

    #[test]
    fn selected_vertical_dependency_reconnects_from_the_top_destination_port() {
        let moves = Rc::new(RefCell::new(Vec::new()));
        let mut state = state_recording_moves(&moves);
        let reconnected = Rc::new(RefCell::new(Vec::new()));
        state.callbacks.on_reconnect = {
            let reconnected = reconnected.clone();
            Rc::new(move |from, old, new| reconnected.borrow_mut().push((from, old, new)))
        };
        let mut data = data_with("source", (80.0, 120.0));
        for (id, y) in [("target", 300.0), ("other", 480.0)] {
            let mut next = data_with(id, (80.0, y));
            if id == "target" { next.steps[0].dependencies.push("source".into()); }
            data.steps.extend(next.steps);
            data.layout.extend(next.layout);
        }
        state.update(data);
        *state.selected_edge.borrow_mut() = Some(("source".into(), "target".into()));
        state.drag_begin((210.0, 300.0));
        assert!(matches!(*state.interaction.borrow(), Interaction::Reconnect { anchor: (210.0, 216.0), .. }));
        state.drag_update((0.0, 175.0));
        state.drag_end();
        assert_eq!(*reconnected.borrow(), vec![("source".to_owned(), "target".to_owned(), "other".to_owned())]);
        assert!(moves.borrow().is_empty());
    }

    /// P2-6: `drag_end` captura la posición FINAL del nodo arrastrado ANTES
    /// de aplicar los datos diferidos (que reconstruyen el modelo desde el
    /// layout persistido, todavía viejo), la re-aplica al modelo nuevo y
    /// persiste ESAS coordenadas — no las del bombeo.
    #[test]
    fn drag_end_persiste_las_coordenadas_finales_con_datos_diferidos() {
        let moves: Rc<RefCell<Vec<(String, f64, f64)>>> = Rc::new(RefCell::new(Vec::new()));
        let state = state_recording_moves(&moves);

        // El «store» aún no sabe nada del drag: layout con la posición vieja.
        let stale = data_with("s1", (80.0, 120.0));
        state.update(stale.clone());

        // Drag en curso: el nodo vive ya en (400, 300)…
        *state.interaction.borrow_mut() = Interaction::Node {
            id: "s1".to_owned(),
            offset: (0.0, 0.0),
            start: (0.0, 0.0),
            moved: true,
        };
        state.model.borrow_mut().nodes[0].position = (400.0, 300.0);
        // …y mientras tanto un refresco con el layout viejo queda diferido.
        state.update(stale);
        assert!(state.deferred.borrow().is_some());

        state.drag_end();

        assert_eq!(state.model.borrow().nodes[0].position, (400.0, 300.0));
        assert_eq!(
            *moves.borrow(),
            vec![("s1".to_owned(), 400.0, 300.0)],
            "se persiste la posición final, no la del layout viejo"
        );
        assert!(state.deferred.borrow().is_none());
        assert!(matches!(*state.interaction.borrow(), Interaction::None));
    }

    /// P2-6 / A9: la rejilla se dibuja a través de la MISMA transformada que
    /// los nodos — con `zoom = 2` sus puntos llegan al borde derecho de la
    /// superficie (pintados «al mango» quedarían recortados a la mitad del
    /// mundo visible, es decir, a la izquierda de `0.75·ancho`).
    #[test]
    fn la_rejilla_pasa_por_la_transformada_compartida() {
        const WIDTH: i32 = 1000;
        const HEIGHT: i32 = 800;
        let moves: Rc<RefCell<Vec<(String, f64, f64)>>> = Rc::new(RefCell::new(Vec::new()));
        let state = state_recording_moves(&moves);
        state.update(data_with("s1", (80.0, 120.0)));
        state.transform.set(ViewTransform {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 2.0,
        });

        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, WIDTH, HEIGHT).expect("superficie");
        let cr = cairo::Context::new(&surface).expect("contexto");
        state.draw(&cr, WIDTH as f64, HEIGHT as f64);
        drop(cr);

        // Detect grid contrast against the canvas background rather than
        // requiring opaque dots; their opacity is a presentation choice.
        let (r, g, b) = crate::pipeline::canvas_model::hex_rgb(Palette::LIGHT.code_bg);
        let background = ((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8);
        let data = surface.data().expect("datos de la superficie");
        let mut found = false;
        for y in 0..HEIGHT as usize {
            for x in 750..WIDTH as usize {
                let at = (y * WIDTH as usize + x) * 4;
                let pixel = if cfg!(target_endian = "little") {
                    (data[at + 2], data[at + 1], data[at])
                } else {
                    (data[at + 1], data[at + 2], data[at + 3])
                };
                if pixel.0.abs_diff(background.0) > 12 || pixel.1.abs_diff(background.1) > 12 || pixel.2.abs_diff(background.2) > 12 {
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
        assert!(
            found,
            "sin puntos de rejilla a la derecha de 0.75·ancho: la rejilla \
             no pasa por la transformada (A9)"
        );
    }

    /// El lienzo se construye y acepta datos (montaje; requiere sesión
    /// gráfica — patrón de minmark).
    #[test]
    #[ignore = "requiere sesión gráfica GTK"]
    fn lienzo_se_construye_y_refresca() {
        assert!(gtk::init().is_ok());
        let no_op: Rc<dyn Fn(String)> = Rc::new(|_value: String| {});
        let no_op2: Rc<dyn Fn(String, String)> = Rc::new(|_a: String, _b: String| {});
        let no_op3: Rc<dyn Fn(String, String, String)> =
            Rc::new(|_a: String, _b: String, _c: String| {});
        let no_op4: Rc<dyn Fn(String, f64, f64)> =
            Rc::new(|_a: String, _b: f64, _c: f64| {});
        let canvas = PipelineCanvas::new(
            Palette::LIGHT,
            CanvasCallbacks {
                on_select: Rc::clone(&no_op),
                on_toggle: no_op,
                on_connect: no_op2.clone(),
                on_disconnect: no_op2,
                on_reconnect: no_op3,
                on_move: no_op4,
                on_place: Rc::new(|_, _| {}),
                on_cancel_brush: Rc::new(|| {}),
                edits_enabled: Rc::new(|| true),
            },
        );
        // Sin datos, el estado vacío del §6 aparece al primer `update` (el
        // contexto por defecto Closed: sin fuente de pipeline).
        canvas.update(CanvasData::default());
        assert!(canvas.empty_message().unwrap().contains("No pipeline open"));
        assert_eq!(canvas.transform().zoom, 1.0);
    }
}
