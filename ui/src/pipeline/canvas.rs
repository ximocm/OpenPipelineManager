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
//! (§11 A12: mientras corre, las mutaciones estructurales están
//! deshabilitadas); sí se permiten selección, checkbox y mover nodos (el
//! layout no es estructural).

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
    has_dependency, CanvasData, CanvasModel, Edge, EdgeKind, Node, ViewTransform, CHECKBOX_SIZE,
};

// --- Colores del §6 --------------------------------------------------------

const COLOR_ORDER: &str = "#475569";
const COLOR_OUTPUT: &str = "#0f766e";
const COLOR_DEPENDENCY: &str = "#7c3aed";
const COLOR_DEPENDENCY_SELECTED: &str = "#a78bfa";
const COLOR_RUNNING: &str = "#f59e0b";
const COLOR_CHECKED: &str = "#2563eb";

fn status_color(status: &opm_store::StepStatus) -> &'static str {
    use opm_store::StepStatus;
    match status {
        StepStatus::Pending | StepStatus::Scheduled => "#94a3b8",
        StepStatus::Running => COLOR_RUNNING,
        StepStatus::Ok => "#16a34a",
        StepStatus::Error => "#dc2626",
        StepStatus::Cancelled => "#475569",
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
    Connect(String),
    /// Extremo de destino de arista → reconectar.
    Reconnect(String, String),
    /// Tarjeta → seleccionar + posible movimiento.
    Node(String, (f64, f64)),
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
        start: (f64, f64),
        cursor: (f64, f64),
    },
    Reconnect {
        source: String,
        target: String,
        start: (f64, f64),
        cursor: (f64, f64),
    },
}

impl Interaction {
    fn from_press(press: Press, screen: (f64, f64), start_pan: (f64, f64)) -> Self {
        match press {
            Press::Toggle | Press::Edge => Interaction::None,
            Press::Connect(source) => Interaction::Connect {
                source,
                start: screen,
                cursor: screen,
            },
            Press::Reconnect(source, target) => Interaction::Reconnect {
                source,
                target,
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
    palette: Palette,
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
            selected_edge: RefCell::new(None),
            deferred: RefCell::new(None),
            pointer: Cell::new(None),
            size: Cell::new((0.0, 0.0)),
            animation: RefCell::new(None),
            boot: Instant::now(),
            palette,
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

        // 2) Mango de conexión del nodo → arrastre para conectar (A9; solo
        //    con ediciones habilitadas).
        if self.callbacks.edits() {
            if let Some(index) = model.hit_handle(screen, &transform) {
                let source = model.nodes[index].id.clone();
                drop(model);
                self.press.set(Some(Press::Connect(source)));
                return;
            }
        }

        // 3) Tarjeta → seleccionar paso + posible movimiento.
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

        // 4) Extremo de DESTINO de una arista fuera de las tarjetas →
        //    reconectar (remove+add, A9).
        if self.callbacks.edits() {
            if let Some((index, false)) = model.hit_edge_endpoint(screen, &transform) {
                let edge = model.edges[index].clone();
                drop(model);
                *self.selected_edge.borrow_mut() =
                    Some((edge.source.clone(), edge.target.clone()));
                self.press.set(Some(Press::Reconnect(edge.source, edge.target)));
                self.queue();
                return;
            }
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
        self.press.set(Some(Press::Pan));
        self.queue();
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
        if let Some((source, target)) = self.selected_edge.borrow().clone() {
            if !has_dependency(&model, &source, &target) {
                self.selected_edge.borrow_mut().take();
            }
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
        let palette = self.palette;
        // Suelo del canvas dentro del panel (§6 «borde + cabecera»): relleno
        // plano apenas tintado —distinto del fondo de la ventana que asoma
        // en el inset del `opm-panel-body`— con la rejilla de puntos encima.
        let (bg_r, bg_g, bg_b) = super::canvas_model::hex_rgb(palette.bg);
        let (fg_r, fg_g, fg_b) = super::canvas_model::hex_rgb(palette.fg);
        const SURFACE_MIX: f64 = 0.05;
        let _ = cr.set_source_rgb(
            bg_r + (fg_r - bg_r) * SURFACE_MIX,
            bg_g + (fg_g - bg_g) * SURFACE_MIX,
            bg_b + (fg_b - bg_b) * SURFACE_MIX,
        );
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
            let _ = cr.set_source_rgb(r, g, b);
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
        for edge in &model.edges {
            draw_edge(cr, edge, &selected, self.boot);
        }
        draw_pending_link(cr, &self.interaction.borrow(), &model);
        for node in &model.nodes {
            draw_node(cr, node, palette);
        }

        let _ = cr.restore();
    }
}

/// Arista según su clase (A9) + flecha + ámbar animado del destino (§6).
fn draw_edge(cr: &cairo::Context, edge: &Edge, selected: &Option<(String, String)>, boot: Instant) {
    let is_selected = matches!(
        selected,
        Some((source, target)) if source == &edge.source && target == &edge.target
    );
    let base_color = if edge.running {
        COLOR_RUNNING
    } else {
        match edge.kind {
            EdgeKind::Order => COLOR_ORDER,
            EdgeKind::Output => COLOR_OUTPUT,
            EdgeKind::Dependency => {
                if is_selected {
                    COLOR_DEPENDENCY_SELECTED
                } else {
                    COLOR_DEPENDENCY
                }
            }
        }
    };
    let (r, g, b) = super::canvas_model::hex_rgb(base_color);
    let _ = cr.set_source_rgb(r, g, b);
    cr.set_line_width(match (edge.running, is_selected) {
        (true, _) => 2.8,
        (_, true) => 3.4,
        _ => match edge.kind {
            EdgeKind::Order => 2.2,
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
    if edge.running {
        // Borde ámbar animado (fase móvil): el timer de `sync_animation`
        // repinta mientras hay `running`.
        let phase = boot.elapsed().as_secs_f64() * 40.0;
        let dashes: [f64; 2] = match edge.kind {
            EdgeKind::Dependency => [10.0, 7.0],
            _ => [8.0, 6.0],
        };
        cr.set_dash(&dashes, -phase);
    } else if edge.kind == EdgeKind::Output {
        // Carril discontinuo teal del SPA (`stroke-dasharray: 7 6`).
        cr.set_dash(&[7.0, 6.0], 0.0);
    } else {
        cr.set_dash(&[], 0.0);
    }
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
    arrow_head(cr, edge.to, edge.control2, base_color);
}

/// Línea temporal de conexión/reconexión (nodo→nodo en curso).
fn draw_pending_link(cr: &cairo::Context, interaction: &Interaction, model: &CanvasModel) {
    let (from, to) = match interaction {
        Interaction::Connect { source, cursor, .. } => anchor_of(model, source, *cursor),
        Interaction::Reconnect {
            source, cursor, ..
        } => anchor_of(model, source, *cursor),
        _ => return,
    };
    let Some(from) = from else { return };
    let (r, g, b) = super::canvas_model::hex_rgb(COLOR_DEPENDENCY_SELECTED);
    let _ = cr.set_source_rgb(r, g, b);
    cr.set_line_width(2.4);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.move_to(from.0, from.1);
    cr.line_to(to.0, to.1);
    cr.set_dash(&[6.0, 5.0], 0.0);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
}

fn anchor_of(model: &CanvasModel, source: &str, cursor: (f64, f64)) -> (Option<(f64, f64)>, (f64, f64)) {
    let anchor = model
        .node_index(source)
        .map(|index| model.nodes[index].source_anchor());
    (anchor, cursor)
}

/// Tarjeta: fondo redondeado, checkbox, id, pastilla de estado y nombre.
fn draw_node(cr: &cairo::Context, node: &Node, palette: Palette) {
    let (x, y, width, height) = node.rect();

    // Fondo + borde (activa → resaltada).
    let (r, g, b) = super::canvas_model::hex_rgb(palette.code_bg);
    rounded_rect(cr, x, y, width, height, 9.0);
    let _ = cr.set_source_rgb(r, g, b);
    let _ = cr.fill_preserve();
    let (br, bg_, bb) = super::canvas_model::hex_rgb(if node.active {
        palette.fg
    } else {
        palette.rule
    });
    cr.set_line_width(if node.active { 2.2 } else { 1.2 });
    let _ = cr.set_source_rgb(br, bg_, bb);
    let _ = cr.stroke();

    // Checkbox de selección.
    let (cx, cy, cs) = (
        x + super::canvas_model::CHECKBOX_OFFSET.0,
        y + super::canvas_model::CHECKBOX_OFFSET.1,
        CHECKBOX_SIZE,
    );
    rounded_rect(cr, cx, cy, cs, cs, 3.0);
    if node.checked {
        let (r, g, b) = super::canvas_model::hex_rgb(COLOR_CHECKED);
        let _ = cr.set_source_rgb(r, g, b);
        let _ = cr.fill_preserve();
        let _ = cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.move_to(cx + 3.0, cy + 7.5);
        cr.line_to(cx + 6.0, cy + 10.5);
        cr.line_to(cx + 11.0, cy + 4.0);
        let _ = cr.stroke();
    } else {
        let (r, g, b) = super::canvas_model::hex_rgb(palette.muted);
        let _ = cr.set_source_rgb(r, g, b);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }

    // Pastilla de estado (derecha de la cabecera).
    let status_text = status_label(&node.status);
    let _ = cr.select_font_face(
        "Sans",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    cr.set_font_size(10.0);
    let pill_width = cr
        .text_extents(status_text)
        .map(|extents| extents.width())
        .unwrap_or(30.0)
        + 14.0;
    let pill_height = 17.0;
    let pill_x = x + width - 10.0 - pill_width;
    let pill_y = y + 12.0;
    rounded_rect(
        cr,
        pill_x,
        pill_y,
        pill_width,
        pill_height,
        pill_height / 2.0,
    );
    let (r, g, b) = super::canvas_model::hex_rgb(status_color(&node.status));
    let _ = cr.set_source_rgb(r, g, b);
    let _ = cr.fill_preserve();
    let _ = cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.stroke();
    if let Ok(extents) = cr.text_extents(status_text) {
        cr.move_to(
            pill_x + (pill_width - extents.width()) / 2.0,
            pill_y + pill_height / 2.0 + extents.height() / 2.0 - 1.0,
        );
        let _ = cr.show_text(status_text);
    }

    // Id (tras el checkbox, hasta la pastilla).
    cr.set_font_size(10.5);
    let id_x = x + CHECKBOX_SIZE + super::canvas_model::CHECKBOX_OFFSET.0 + 22.0;
    let id_text = fit_text(cr, &node.id, (pill_x - id_x).max(20.0));
    let (r, g, b) = super::canvas_model::hex_rgb(palette.muted);
    let _ = cr.set_source_rgb(r, g, b);
    cr.move_to(id_x, y + 26.0);
    let _ = cr.show_text(&id_text);

    // Nombre (segunda fila).
    cr.set_font_size(13.5);
    let name_text = fit_text(cr, &node.name, width - 24.0);
    let (r, g, b) = super::canvas_model::hex_rgb(palette.fg);
    let _ = cr.set_source_rgb(r, g, b);
    cr.move_to(x + 12.0, y + 66.0);
    let _ = cr.show_text(&name_text);
}

/// Flecha en el extremo `tip` apuntando desde `from`.
fn arrow_head(cr: &cairo::Context, tip: (f64, f64), from: (f64, f64), color: &str) {
    let angle = (tip.1 - from.1).atan2(tip.0 - from.0);
    let spread = 0.52;
    let length = 9.0;
    let back1 = angle + std::f64::consts::PI - spread;
    let back2 = angle + std::f64::consts::PI + spread;
    let (r, g, b) = super::canvas_model::hex_rgb(color);
    let _ = cr.set_source_rgb(r, g, b);
    cr.move_to(tip.0, tip.1);
    cr.line_to(tip.0 + back1.cos() * length, tip.1 + back1.sin() * length);
    cr.line_to(tip.0 + back2.cos() * length, tip.1 + back2.sin() * length);
    cr.close_path();
    let _ = cr.fill();
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
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_focusable(true);
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
            click.connect_pressed(move |gesture, _n_press, x, y| {
                if gesture.current_button() != gdk::BUTTON_PRIMARY {
                    return;
                }
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
            keys.connect_key_pressed(move |_, key, _code, _modifiers| {
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
            project_path: "/proy".to_owned(),
            has_pipeline: true,
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
                edits_enabled: Rc::new(|| true),
            },
            Palette::LIGHT,
        )
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

        // Color de rejilla de ESTA paleta (el borde de nodo inactivo usa el
        // mismo tono, pero el nodo vive en x < 700: a la derecha de 750 solo
        // puede haber rejilla).
        let (rule_r, rule_g, rule_b) =
            crate::pipeline::canvas_model::hex_rgb(Palette::LIGHT.rule);
        let rule = (
            (rule_r * 255.0).round() as u8,
            (rule_g * 255.0).round() as u8,
            (rule_b * 255.0).round() as u8,
        );
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
                if pixel == rule {
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
                edits_enabled: Rc::new(|| true),
            },
        );
        // Sin datos, el estado vacío del §6 aparece al primer `update` (el
        // modelo arranca en `CanvasModel::default()`, sin mensaje).
        canvas.update(CanvasData::default());
        assert!(canvas.empty_message().unwrap().contains("No project open"));
        assert_eq!(canvas.transform().zoom, 1.0);
    }
}
