//! Lienzo del pipeline (§6 `pipeline/canvas.rs`, A9): grafo en un
//! `gtk::DrawingArea` propio con transformada única para render, drag e
//! hit-testing.
//!
//! Modelo puro (testable sin display):
//!
//! - posiciones de [`CanvasData::build`] con fallback `x=80+i·330, y=120`
//!   cuando `visual_layout` no tiene la entrada;
//! - **tres clases de arista** (A9): *order* (pasos consecutivos, recta
//!   gris, inmutable), *output* (`input.source_step → paso`, teal discontinua
//!   en carril curvo, inmutable) y *dependency* (entradas EXPLÍCITAS de
//!   `target.dependencies` no implicadas por los enlaces de input — la
//!   storage las rederiva en cada carga, así que exponerlas como editables
//!   sería inútil —, sólidas, seleccionables y borrables con Delete);
//! - hit-testing con tolerancia en **espacio de pantalla** sobre la misma
//!   [`ViewTransform`] que usa el render.
//!
//! La capa widget ([`PipelineCanvas`]) añade gestos (pan, zoom con
//! ctrl+rueda, drag de nodo con persistencia del NODO MOVIDO, arrastre
//! nodo→nodo para conectar, reconectar arista, Delete/Backspace) y el
//! refresco animado del borde ámbar mientras algo corre (§11 A12).

use std::collections::HashMap;

use opm_pipeline::PipelineStep;
use opm_store::{Position, ProjectSnapshot, StepState, StepStatus};

// --- Constantes geométricas (§6: fallback x=80+i·330, y=120) ---------------

pub const NODE_START_X: f64 = 80.0;
pub const NODE_START_Y: f64 = 120.0;
pub const NODE_GAP_X: f64 = 330.0;
/// Ancho de la tarjeta de nodo (menor que el paso de 330 para que las
/// aristas rectas quepan).
pub const NODE_WIDTH: f64 = 260.0;
pub const NODE_HEIGHT: f64 = 96.0;
/// Tolerancia de hit-testing de aristas en px de pantalla (A9).
pub const EDGE_TOLERANCE_PX: f64 = 9.0;
/// Tolerancia de zonas de conexión (mango de origen / extremos de arista).
pub const HANDLE_TOLERANCE_PX: f64 = 12.0;
/// Checkbox de selección del nodo (esquina superior izquierda).
pub const CHECKBOX_OFFSET: (f64, f64) = (13.0, 16.0);
pub const CHECKBOX_SIZE: f64 = 14.0;

// --- Transformada única -----------------------------------------------------

/// Único sistema de coordenadas para render/drag/hit-test (A9):
/// `pantalla = mundo * zoom + pan`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
        }
    }
}

impl ViewTransform {
    pub const MIN_ZOOM: f64 = 0.35;
    pub const MAX_ZOOM: f64 = 2.5;

    pub fn to_screen(&self, world: (f64, f64)) -> (f64, f64) {
        (
            world.0 * self.zoom + self.pan_x,
            world.1 * self.zoom + self.pan_y,
        )
    }

    pub fn to_world(&self, screen: (f64, f64)) -> (f64, f64) {
        (
            (screen.0 - self.pan_x) / self.zoom,
            (screen.1 - self.pan_y) / self.zoom,
        )
    }

    /// Zoom alrededor de `screen` (ctrl+rueda): el punto del mundo bajo el
    /// cursor NO se mueve.
    pub fn zoom_at(&mut self, screen: (f64, f64), factor: f64) {
        let world = self.to_world(screen);
        let zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        if zoom == self.zoom {
            return;
        }
        self.zoom = zoom;
        self.pan_x = screen.0 - world.0 * self.zoom;
        self.pan_y = screen.1 - world.1 * self.zoom;
    }
}

// --- Modelo puro -----------------------------------------------------------

/// Clase de arista (A9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Pasos consecutivos: recta gris `#475569`, inmutable.
    Order,
    /// `input.source_step → paso`: teal `#0f766e` discontinua en carril
    /// curvo, inmutable.
    Output,
    /// Dependencia explícita no implicada por inputs: sólida, seleccionable
    /// y borrable.
    Dependency,
}

/// Arista ya clasificada con su geometría (puntos de control del cubic).
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub kind: EdgeKind,
    pub source: String,
    pub target: String,
    /// Clave del output origen (aristas [`EdgeKind::Output`]).
    pub output: Option<String>,
    /// El paso destino está `running` (borde ámbar animado, §6).
    pub running: bool,
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub control1: (f64, f64),
    pub control2: (f64, f64),
}

impl Edge {
    /// ¿Es seleccionable/borrable (solo dependencias, A9)?
    pub fn selectable(&self) -> bool {
        self.kind == EdgeKind::Dependency
    }

    /// Muestreo del trazado en coordenadas de mundo (hit-testing y flecha).
    pub fn sample(&self, steps: usize) -> Vec<(f64, f64)> {
        let steps = steps.max(2);
        (0..=steps)
            .map(|index| {
                let t = index as f64 / steps as f64;
                cubic_point(
                    self.from,
                    self.control1,
                    self.control2,
                    self.to,
                    t,
                )
            })
            .collect()
    }

    /// Ancla de origen (centro derecho del nodo origen).
    pub fn source_anchor(&self) -> (f64, f64) {
        self.from
    }

    /// Ancla de destino (centro izquierdo del nodo destino).
    pub fn target_anchor(&self) -> (f64, f64) {
        self.to
    }
}

/// Nodo de la vista.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub position: (f64, f64),
    pub status: StepStatus,
    pub checked: bool,
    pub active: bool,
}

impl Node {
    pub fn rect(&self) -> (f64, f64, f64, f64) {
        (
            self.position.0,
            self.position.1,
            NODE_WIDTH,
            NODE_HEIGHT,
        )
    }

    pub fn source_anchor(&self) -> (f64, f64) {
        (self.position.0 + NODE_WIDTH, self.position.1 + NODE_HEIGHT / 2.0)
    }

    pub fn target_anchor(&self) -> (f64, f64) {
        (self.position.0, self.position.1 + NODE_HEIGHT / 2.0)
    }

    /// Rectángulo del checkbox en coordenadas de mundo.
    pub fn checkbox_rect(&self) -> (f64, f64, f64, f64) {
        (
            self.position.0 + CHECKBOX_OFFSET.0,
            self.position.1 + CHECKBOX_OFFSET.1,
            CHECKBOX_SIZE,
            CHECKBOX_SIZE,
        )
    }

    pub fn contains(&self, world: (f64, f64)) -> bool {
        let (x, y, w, h) = self.rect();
        world.0 >= x && world.0 <= x + w && world.1 >= y && world.1 <= y + h
    }
}

/// Datos de entrada del modelo (se construyen desde el snapshot del store).
#[derive(Clone, Debug, Default)]
pub struct CanvasData {
    /// `project_path` del snapshot: vacío = no hay proyecto abierto.
    pub project_path: String,
    /// ¿El proyecto tiene pipeline importado?
    pub has_pipeline: bool,
    pub steps: Vec<PipelineStep>,
    pub states: HashMap<String, StepState>,
    pub layout: HashMap<String, Position>,
    pub selected: Option<String>,
}

impl CanvasData {
    /// Construye los datos desde el snapshot del store + la selección actual.
    pub fn new(snapshot: &ProjectSnapshot, selected: Option<&str>) -> Self {
        Self {
            project_path: snapshot.project_path.clone(),
            has_pipeline: snapshot.pipeline.is_some(),
            steps: snapshot
                .pipeline
                .as_ref()
                .map(|pipeline| pipeline.steps.clone())
                .unwrap_or_default(),
            states: snapshot.state.clone(),
            layout: snapshot.visual_layout.clone(),
            selected: selected.map(str::to_owned),
        }
    }

    /// Mensaje del estado vacío del lienzo (§6).
    pub fn empty_message(&self) -> Option<String> {
        if self.project_path.is_empty() {
            Some("No project open — use Project ▸ New project or Open project".to_owned())
        } else if !self.has_pipeline {
            Some("No pipeline imported — use Import pipeline".to_owned())
        } else {
            None
        }
    }
}

/// Grafo dibujado: nodos + aristas clasificadas.
#[derive(Clone, Debug, Default)]
pub struct CanvasModel {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub empty: Option<String>,
}

impl CanvasModel {
    /// Posición por defecto del paso `index` (§6: `x=80+i·330, y=120`).
    pub fn default_position(index: usize) -> (f64, f64) {
        (
            NODE_START_X + index as f64 * NODE_GAP_X,
            NODE_START_Y,
        )
    }

    /// Construye nodos (posición desde `visual_layout`, si no el fallback)
    /// y las tres clases de arista (A9).
    pub fn build(data: &CanvasData) -> Self {
        let mut nodes = Vec::with_capacity(data.steps.len());
        for (index, step) in data.steps.iter().enumerate() {
            let state = data.states.get(&step.id);
            let position = data
                .layout
                .get(&step.id)
                .map(|position| (position.x, position.y))
                .unwrap_or_else(|| Self::default_position(index));
            nodes.push(Node {
                id: step.id.clone(),
                name: if step.name.is_empty() {
                    step.id.clone()
                } else {
                    step.name.clone()
                },
                position,
                status: state.map(|state| state.status).unwrap_or_default(),
                checked: state.map(|state| state.selected).unwrap_or(false),
                active: data.selected.as_deref() == Some(step.id.as_str()),
            });
        }
        let edges = classify_edges(&data.steps, |id| {
            data.states
                .get(id)
                .map(|state| state.status)
                .unwrap_or_default()
        });
        let mut model = Self {
            nodes,
            edges,
            empty: None,
        };
        model.recompute_geometry(data);
        model.empty = data.empty_message();
        model
    }

    /// Recalcula la geometría de las aristas desde las posiciones actuales
    /// de los nodos (tras un drag en curso o un movimiento).
    pub fn recompute_geometry(&mut self, data: &CanvasData) {
        let index_of = |id: &str| data.steps.iter().position(|step| step.id == id);
        for edge in &mut self.edges {
            let Some(source) = self.nodes.iter().find(|node| node.id == edge.source) else {
                continue;
            };
            let Some(target) = self.nodes.iter().find(|node| node.id == edge.target) else {
                continue;
            };
            edge.running = data
                .states
                .get(&edge.target)
                .map(|state| state.status == StepStatus::Running)
                .unwrap_or(false);
            edge.from = source.source_anchor();
            edge.to = target.target_anchor();
            let skip = match (index_of(&edge.source), index_of(&edge.target)) {
                (Some(a), Some(b)) => (a.abs_diff(b)).max(1),
                _ => 1,
            };
            // El arco se escala con la distancia: un lift fijo para saltos
            // cortos crea una aguja (los puntos de control casi coinciden).
            let dx = (edge.to.0 - edge.from.0).abs().max(1.0);
            let bend = match edge.kind {
                EdgeKind::Order => 0.0,
                EdgeKind::Output => (dx * 0.32 + skip as f64 * 18.0).clamp(36.0, 180.0),
                EdgeKind::Dependency => (dx * 0.30 + skip as f64 * 16.0).clamp(32.0, 160.0),
            };
            let (control1, control2) = if edge.kind == EdgeKind::Order {
                // Recta: puntos de control sobre la línea (muestreo lineal).
                (
                    lerp(edge.from, edge.to, 1.0 / 3.0),
                    lerp(edge.from, edge.to, 2.0 / 3.0),
                )
            } else {
                curved_controls(edge.from, edge.to, bend)
            };
            edge.control1 = control1;
            edge.control2 = control2;
        }
    }

    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    /// ¿Qué nodo contiene el punto de mundo? (el primero; las tarjetas no
    /// se solapan en el layout por defecto).
    pub fn node_at(&self, world: (f64, f64)) -> Option<usize> {
        self.nodes.iter().position(|node| node.contains(world))
    }

    /// ¿El punto de mundo cae en el checkbox del nodo `index`?
    pub fn hit_checkbox(&self, index: usize, world: (f64, f64)) -> bool {
        let Some(node) = self.nodes.get(index) else {
            return false;
        };
        let (x, y, w, h) = node.checkbox_rect();
        world.0 >= x && world.0 <= x + w && world.1 >= y && world.1 <= y + h
    }

    /// Nodo cuyo mango de conexión (centro derecho) está a ≤ tolerancia de
    /// pantalla del punto — en **espacio de pantalla** (A9).
    pub fn hit_handle(
        &self,
        screen: (f64, f64),
        transform: &ViewTransform,
    ) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| {
                let anchor = transform.to_screen(node.source_anchor());
                distance(anchor, screen) <= HANDLE_TOLERANCE_PX
            })
    }

    /// Arista de dependencia cuyo trazado pasa a ≤ tolerancia de pantalla
    /// del punto (muestreo → coordenadas de pantalla, A9).
    pub fn hit_edge(
        &self,
        screen: (f64, f64),
        transform: &ViewTransform,
    ) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (index, edge) in self.edges.iter().enumerate() {
            if !edge.selectable() {
                continue;
            }
            let mut min = f64::INFINITY;
            let points = edge.sample(40);
            for pair in points.windows(2) {
                let a = transform.to_screen(pair[0]);
                let b = transform.to_screen(pair[1]);
                let distance = point_segment_distance(screen, a, b);
                if distance < min {
                    min = distance;
                }
            }
            if min <= EDGE_TOLERANCE_PX && best.map(|(_, m)| min < m).unwrap_or(true) {
                best = Some((index, min));
            }
        }
        best.map(|(index, _)| index)
    }

    /// Extremo de arista de dependencia cerca del punto (para reconectar):
    /// `Some((índice de arista, extremo))`.
    pub fn hit_edge_endpoint(
        &self,
        screen: (f64, f64),
        transform: &ViewTransform,
    ) -> Option<(usize, bool)> {
        // true = extremo de origen, false = destino.
        let mut best: Option<(usize, bool, f64)> = None;
        for (index, edge) in self.edges.iter().enumerate() {
            if !edge.selectable() {
                continue;
            }
            let source = transform.to_screen(edge.source_anchor());
            let target = transform.to_screen(edge.target_anchor());
            let ds = distance(source, screen);
            if ds <= HANDLE_TOLERANCE_PX && best.map(|(_, _, m)| ds < m).unwrap_or(true) {
                best = Some((index, true, ds));
            }
            let dt = distance(target, screen);
            if dt <= HANDLE_TOLERANCE_PX && best.map(|(_, _, m)| dt < m).unwrap_or(true) {
                best = Some((index, false, dt));
            }
        }
        best.map(|(index, source, _)| (index, source))
    }

    /// Aristas de dependencia seleccionables (solo pruebas; el trazado y el
    /// hit-testing filtran con [`Edge::selectable`] directamente).
    #[cfg(test)]
    pub fn dependency_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(|edge| edge.selectable())
    }
}

/// ¿El par (source→target) de una arista de dependencia existe en el
/// modelo? (la clave de selección tras un refresco).
pub fn has_dependency(model: &CanvasModel, source: &str, target: &str) -> bool {
    model
        .edges
        .iter()
        .any(|edge| edge.selectable() && edge.source == source && edge.target == target)
}

/// Clasificación de aristas (A9) sobre los pasos: order consecutivo, output
/// por enlaces `source_step/source_output` y dependencias EXPLÍCITAS no
/// implicadas por esos enlaces. `status_of` aporta el `running` del destino.
///
/// Una dependencia `d` de `t` está **implícita** cuando `t` tiene un input
/// con `source_step = d` y `source_output` que resuelve a un output
/// conocido de `d` — exactamente lo que `opm-store::resolve_input_sources`
/// rederiva en cada carga; esas aristas NO se exponen como editables.
pub fn classify_edges(
    steps: &[PipelineStep],
    status_of: impl Fn(&str) -> StepStatus,
) -> Vec<Edge> {
    let known = |id: &str| steps.iter().any(|step| step.id == id);
    let mut edges: Vec<Edge> = Vec::new();

    // 1) Order: consecutivos.
    for pair in steps.windows(2) {
        edges.push(Edge {
            kind: EdgeKind::Order,
            source: pair[0].id.clone(),
            target: pair[1].id.clone(),
            output: None,
            running: false,
            from: (0.0, 0.0),
            to: (0.0, 0.0),
            control1: (0.0, 0.0),
            control2: (0.0, 0.0),
        });
    }

    // 2) Output: input con source_step + source_output (paridad SPA).
    for step in steps {
        for input in &step.inputs {
            let (Some(source_step), Some(source_output)) =
                (&input.source_step, &input.source_output)
            else {
                continue;
            };
            if source_step.is_empty() || source_output.is_empty() || !known(source_step) {
                continue;
            }
            let key = (
                source_step.clone(),
                step.id.clone(),
                source_output.clone(),
            );
            if edges.iter().any(|edge| {
                edge.kind == EdgeKind::Output
                    && (
                        (edge.source.clone(), edge.target.clone(), edge.output.clone().unwrap_or_default()) == key
                    )
            }) {
                continue;
            }
            edges.push(Edge {
                kind: EdgeKind::Output,
                source: source_step.clone(),
                target: step.id.clone(),
                output: Some(source_output.clone()),
                running: false,
                from: (0.0, 0.0),
                to: (0.0, 0.0),
                control1: (0.0, 0.0),
                control2: (0.0, 0.0),
            });
        }
    }

    // 3) Dependency: entradas explícitas de `target.dependencies` que NO
    //    estén implicadas por los enlaces de input (A9).
    for step in steps {
        for dependency in &step.dependencies {
            if !known(dependency) || dependency == &step.id {
                continue;
            }
            if implied_by_input_sources(steps, &step.id, dependency) {
                continue;
            }
            if edges.iter().any(|edge| {
                edge.kind == EdgeKind::Dependency
                    && edge.source == *dependency
                    && edge.target == step.id
            }) {
                continue;
            }
            edges.push(Edge {
                kind: EdgeKind::Dependency,
                source: dependency.clone(),
                target: step.id.clone(),
                output: None,
                running: false,
                from: (0.0, 0.0),
                to: (0.0, 0.0),
                control1: (0.0, 0.0),
                control2: (0.0, 0.0),
            });
        }
    }

    // Estado `running` del destino (borde ámbar, §6).
    for edge in &mut edges {
        edge.running = status_of(&edge.target) == StepStatus::Running;
    }
    edges
}

/// ¿La dependencia `dep` de `target` está implicada por un enlace de input?
/// (misma condición que `resolve_input_sources` de opm-store: enlace no
/// vacío + output del paso origen cuya clave O path coincide).
pub fn implied_by_input_sources(steps: &[PipelineStep], target: &str, dep: &str) -> bool {
    let Some(target_step) = steps.iter().find(|step| step.id == target) else {
        return false;
    };
    let Some(source_step) = steps.iter().find(|step| step.id == dep) else {
        return false;
    };
    target_step.inputs.iter().any(|input| {
        input.source_step.as_deref() == Some(dep)
            && input
                .source_output
                .as_deref()
                .is_some_and(|output| !output.is_empty())
            && {
                let wanted = input.source_output.as_deref().unwrap_or_default();
                source_step
                    .outputs
                    .iter()
                    .any(|output| output.key == wanted || output.path == wanted)
            }
    })
}

// --- Geometría auxiliar ----------------------------------------------------

fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Puntos de control del cubico del port de `curvedPath` (SPA).
fn curved_controls(
    source: (f64, f64),
    target: (f64, f64),
    vertical_bend: f64,
) -> ((f64, f64), (f64, f64)) {
    let dx = target.0 - source.0;
    let direction = if dx >= 0.0 { 1.0 } else { -1.0 };
    let distance = dx.abs().max(1.0);
    let horizontal_bend = (distance * 0.38).clamp(32.0, 220.0);
    (
        (
            source.0 + direction * horizontal_bend,
            source.1 - vertical_bend,
        ),
        (
            target.0 - direction * horizontal_bend,
            target.1 - vertical_bend,
        ),
    )
}

/// Punto de la cúbica `B(t)`.
fn cubic_point(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    t: f64,
) -> (f64, f64) {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Distancia punto→segmento (hit-testing de aristas en pantalla).
fn point_segment_distance(
    point: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
) -> f64 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (point.0 - a.0, point.1 - a.1);
    let length_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if length_sq <= f64::EPSILON {
        return distance(point, a);
    }
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / length_sq).clamp(0.0, 1.0);
    distance(point, (a.0 + ab.0 * t, a.1 + ab.1 * t))
}

/// `#rrggbb` → componentes 0..1 (colores del §6 y la paleta).
pub fn hex_rgb(hex: &str) -> (f64, f64, f64) {
    let digits = hex.trim_start_matches('#');
    if digits.len() != 6 {
        return (0.0, 0.0, 0.0);
    }
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&digits[range], 16)
            .map(|value| value as f64 / 255.0)
            .unwrap_or(0.0)
    };
    (channel(0..2), channel(2..4), channel(4..6))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opm_pipeline::{FieldType, OutputSpec, ValueSpec};

    fn step(id: &str) -> PipelineStep {
        PipelineStep {
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
        }
    }

    fn data(steps: Vec<PipelineStep>) -> CanvasData {
        CanvasData {
            project_path: "/proy".to_owned(),
            has_pipeline: true,
            steps,
            states: HashMap::new(),
            layout: HashMap::new(),
            selected: None,
        }
    }

    /// Fallback de posición §6: `x=80+i·330, y=120`; `visual_layout`
    /// manda cuando existe.
    #[test]
    fn posiciones_con_fallback() {
        assert_eq!(
            CanvasModel::default_position(0),
            (NODE_START_X, NODE_START_Y)
        );
        assert_eq!(CanvasModel::default_position(3), (80.0 + 3.0 * 330.0, 120.0));

        let mut data = data(vec![step("a"), step("b")]);
        data.layout.insert(
            "b".into(),
            Position {
                x: 10.0,
                y: 20.0,
            },
        );
        let model = CanvasModel::build(&data);
        assert_eq!(model.nodes[0].position, (80.0, 120.0));
        assert_eq!(model.nodes[1].position, (10.0, 20.0));
    }

    /// Clasificación de aristas (A9): order consecutivo + outputs; las
    /// dependencias IMPLICADAS por enlaces de input quedan EXCLUIDAS (la
    /// storage las rederiva: editarlas sería inútil).
    #[test]
    fn clasificacion_de_aristas_a9() {
        let mut prepare = step("prepare");
        prepare.outputs.push(OutputSpec {
            key: "csv".into(),
            label: None,
            path: "outputs/input.csv".into(),
        });
        let mut analyze = step("analyze");
        analyze.inputs.push(ValueSpec {
            key: "input_file".into(),
            label: None,
            kind: FieldType::File,
            required: true,
            default: None,
            options: Vec::new(),
            min: None,
            max: None,
            description: String::new(),
            source_step: Some("prepare".into()),
            source_output: Some("csv".into()),
        });
        // Explícita pero implicada por el enlace de input → NO dibujable.
        analyze.dependencies.push("prepare".into());
        let mut summarize = step("summarize");
        // Dependencia explícita NO implicada (sin input con enlace) → sí.
        summarize.dependencies.push("prepare".into());
        analyze.outputs.push(OutputSpec {
            key: "report".into(),
            label: None,
            path: "outputs/report.txt".into(),
        });

        let edges = classify_edges(
            &[prepare, analyze, summarize],
            |_| StepStatus::Pending,
        );
        let orders: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Order)
            .map(|edge| (edge.source.as_str(), edge.target.as_str()))
            .collect();
        assert_eq!(orders, vec![("prepare", "analyze"), ("analyze", "summarize")]);

        let outputs: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Output)
            .map(|edge| (edge.source.as_str(), edge.target.as_str()))
            .collect();
        assert_eq!(outputs, vec![("prepare", "analyze")]);

        let dependencies: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Dependency)
            .map(|edge| (edge.source.as_str(), edge.target.as_str()))
            .collect();
        assert_eq!(
            dependencies,
            vec![("prepare", "summarize")],
            "la dependencia implícita por input NO se expone; la explícita sin enlace sí"
        );
    }

    /// Dependencia hacia un paso desconocido o autoreferencia → ignorada.
    #[test]
    fn dependencias_desconocidas_ignoradas() {
        let mut a = step("a");
        a.dependencies.push("ghost".into());
        a.dependencies.push("a".into());
        let edges = classify_edges(&[a], |_| StepStatus::Pending);
        assert!(!edges.iter().any(|edge| edge.kind == EdgeKind::Dependency));
    }

    /// El estado `running` del destino tiñe la arista (ámbar, §6).
    #[test]
    fn arista_de_destino_running() {
        let a = step("a");
        let b = step("b");
        let edges = classify_edges(&[a, b], |id| {
            if id == "b" {
                StepStatus::Running
            } else {
                StepStatus::Pending
            }
        });
        assert!(!edges.is_empty());
        assert!(
            edges.iter().all(|edge| edge.running),
            "toda arista con destino `b` está ámbar"
        );
    }

    /// Round-trip de la transformada única con zoom/pan arbitrarios (A9:
    /// UN transform para render, drag e hit-test).
    #[test]
    fn transformada_round_trip() {
        let mut transform = ViewTransform::default();
        transform.pan_x = -37.5;
        transform.pan_y = 12.0;
        transform.zoom = 0.73;
        for point in [(0.0, 0.0), (80.0, 120.0), (-1e3, 4e4), (12.75, 9.5)] {
            let back = transform.to_world(transform.to_screen(point));
            assert!((back.0 - point.0).abs() < 1e-9);
            assert!((back.1 - point.1).abs() < 1e-9);
        }
    }

    /// Zoom anclado: el punto del mundo bajo el cursor no se mueve y los
    /// límites se respetan.
    #[test]
    fn zoom_anclado_en_el_cursor() {
        let mut transform = ViewTransform::default();
        let cursor = (400.0, 300.0);
        let world_before = transform.to_world(cursor);
        transform.zoom_at(cursor, 1.5);
        let world_after = transform.to_world(cursor);
        assert!((world_before.0 - world_after.0).abs() < 1e-9);
        assert!((world_before.1 - world_after.1).abs() < 1e-9);
        for _ in 0..20 {
            transform.zoom_at(cursor, 2.0);
        }
        assert_eq!(transform.zoom, ViewTransform::MAX_ZOOM);
        for _ in 0..40 {
            transform.zoom_at(cursor, 0.5);
        }
        assert_eq!(transform.zoom, ViewTransform::MIN_ZOOM);
    }

    /// Hit-testing de nodos/checkbox en coordenadas de mundo.
    #[test]
    fn hit_testing_de_nodos() {
        let model = CanvasModel::build(&data(vec![step("a"), step("b")]));
        // a: (80,120) 260x96; b: (410,120).
        assert_eq!(model.node_at((100.0, 130.0)), Some(0));
        assert_eq!(model.node_at((420.0, 140.0)), Some(1));
        assert_eq!(model.node_at((0.0, 0.0)), None);
        // Checkbox de a: (93,136) 14x14.
        assert!(model.hit_checkbox(0, (95.0, 140.0)));
        assert!(!model.hit_checkbox(0, (150.0, 180.0)));
    }

    /// Tolerancia de arista en ESPACIO DE PANTALLA (A9): con zoom 0.5 la
    /// misma distancia en mundo vale la mitad en pantalla.
    #[test]
    fn tolerancia_de_arista_en_pantalla() {
        let mut a = step("a");
        a.dependencies.push("b".into());
        let b = step("b");
        let edges = classify_edges(&[a, b], |_| StepStatus::Pending);
        assert_eq!(
            edges.iter().filter(|edge| edge.selectable()).count(),
            1,
            "a→b no está implicada por inputs: es arista de dependencia"
        );

        let data = data(vec![{
            let mut a = step("a");
            a.dependencies.push("b".into());
            a
        }, step("b")]);
        let model = CanvasModel::build(&data);
        assert_eq!(model.dependency_edges().count(), 1);
        let edge = model.dependency_edges().next().unwrap().clone();

        // Punto sobre el trazado (mitad del camino): zoom 1 golpea.
        let mid = edge.sample(16)[8];
        let mut transform = ViewTransform::default();
        assert!(model.hit_edge(transform.to_screen(mid), &transform).is_some());

        // 6px DESVIADO en pantalla → golpea (tolerancia 9).
        let near = (mid.0, mid.1 + 6.0 / transform.zoom);
        assert!(model.hit_edge(transform.to_screen(near), &transform).is_some());

        // Con zoom 0.5, 6px de mundo = 3px de pantalla → sigue golpeando…
        transform.zoom = 0.5;
        let offset_screen = 6.0;
        let world = transform.to_world((transform.to_screen(mid).0, transform.to_screen(mid).1 + offset_screen));
        assert!(model.hit_edge(transform.to_screen(world), &transform).is_some());
        // …pero 30px de pantalla NO (lejos de la tolerancia de 9).
        let far = (transform.to_screen(mid).0, transform.to_screen(mid).1 + 30.0);
        assert!(model.hit_edge(far, &transform).is_none());
    }

    /// El mango de conexión vive en el borde derecho del nodo (tolerancia
    /// de pantalla).
    #[test]
    fn hit_del_mango_de_conexion() {
        let model = CanvasModel::build(&data(vec![step("a")]));
        let node = &model.nodes[0];
        let anchor = node.source_anchor();
        let transform = ViewTransform::default();
        assert_eq!(
            model.hit_handle(transform.to_screen(anchor), &transform),
            Some(0)
        );
        // 40px lejos en pantalla → fuera de tolerancia.
        assert_eq!(model.hit_handle((anchor.0 + 40.0, anchor.1), &transform), None);
    }

    /// Extremos de arista detectables para reconectar (solo dependencias).
    #[test]
    fn hit_de_extremos_para_reconectar() {
        let data = data(vec![{
            let mut a = step("a");
            a.dependencies.push("b".into());
            a
        }, step("b")]);
        let model = CanvasModel::build(&data);
        let edge = model.dependency_edges().next().unwrap().clone();
        let transform = ViewTransform::default();
        // edges = [order a→b, dependencia b→a]: la dependencia es la 1.
        assert_eq!(
            model.hit_edge_endpoint(transform.to_screen(edge.source_anchor()), &transform),
            Some((1, true))
        );
        assert_eq!(
            model.hit_edge_endpoint(transform.to_screen(edge.target_anchor()), &transform),
            Some((1, false))
        );
        assert_eq!(model.hit_edge_endpoint((1.0, 1.0), &transform), None);
    }

    /// Geometría: la recta del order pasa por sus extremos y el curvo del
    /// output se desvía hacia arriba (carril curvo).
    #[test]
    fn geometria_de_aristas() {
        let mut a = step("a");
        a.outputs.push(OutputSpec {
            key: "out".into(),
            label: None,
            path: "o.txt".into(),
        });
        let mut b = step("b");
        b.inputs.push(ValueSpec {
            key: "in".into(),
            label: None,
            kind: FieldType::File,
            required: false,
            default: None,
            options: Vec::new(),
            min: None,
            max: None,
            description: String::new(),
            source_step: Some("a".into()),
            source_output: Some("out".into()),
        });
        let data = data(vec![a, b]);
        let model = CanvasModel::build(&data);
        let order = model.edges.iter().find(|edge| edge.kind == EdgeKind::Order).unwrap();
        assert_eq!(order.from, model.nodes[0].source_anchor());
        assert_eq!(order.to, model.nodes[1].target_anchor());
        let output = model.edges.iter().find(|edge| edge.kind == EdgeKind::Output).unwrap();
        // El carril curvo se eleva por encima de la línea recta.
        assert!(output.control1.1 < output.from.1);
        assert!(output.control2.1 < output.to.1);
        // El muestreo empieza/termina en los anclas.
        let points = output.sample(8);
        assert_eq!(points.first().copied(), Some(output.from));
        assert_eq!(points.last().copied(), Some(output.to));
    }

    /// Estados vacíos del lienzo (§6).
    #[test]
    fn mensajes_de_estado_vacio() {
        let mut data = CanvasData::default();
        assert!(data.empty_message().unwrap().contains("No project open"));
        data.project_path = "/proy".into();
        assert!(data.empty_message().unwrap().contains("No pipeline imported"));
        data.has_pipeline = true;
        assert_eq!(data.empty_message(), None);
    }

    /// Colores hexadecimales.
    #[test]
    fn parse_de_colores_hex() {
        assert_eq!(hex_rgb("#000000"), (0.0, 0.0, 0.0));
        assert_eq!(hex_rgb("ffffff"), (1.0, 1.0, 1.0));
        let (r, g, b) = hex_rgb("#ff8000");
        assert!((r - 1.0).abs() < 1e-9);
        assert!((g - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(b, 0.0);
        assert_eq!(hex_rgb("zzz"), (0.0, 0.0, 0.0));
    }

    /// `has_dependency` confirma la selección tras refrescar.
    #[test]
    fn tiene_dependencia_para_seleccion() {
        let data = data(vec![{
            let mut a = step("a");
            a.dependencies.push("b".into());
            a
        }, step("b")]);
        let model = CanvasModel::build(&data);
        // `a.dependencies = [b]` → la dependencia entra en `a`: la arista
        // seleccionable es `b → a` (dirección de `classify_edges`).
        assert!(has_dependency(&model, "b", "a"));
        assert!(!has_dependency(&model, "a", "b"));
    }
}
