// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Modelo puro del explorador de archivos (ISSUE-022): árbol perezoso por
//! directorio con filtros de tipos documentales y ocultos.
//!
//! - §14.4: por defecto solo tipos documentales (texto, Markdown, YAML, JSON,
//!   shell, FASTA, imágenes, tablas y configuración); `Show All Files`
//!   ([`ExplorerFilters::show_all`]) muestra todo.
//! - §14.5: los ocultos (nombre que empieza por `.`) están ocultos por
//!   defecto; `Show Hidden Files` ([`ExplorerFilters::show_hidden`]) los
//!   muestra. Ambos filtros son independientes.
//! - §14.6: los archivos ignorados por `.gitignore` siguen **visibles** en el
//!   explorador: este modelo no consulta `.gitignore` (su exclusión aplica a
//!   búsqueda/indexado, ISSUE-025/028).
//! - §14.7: los symlinks se listan como entradas, pero las carpetas enlazadas
//!   no se recorren: no son expandibles.
//! - §47.5: carga incremental sin bloquear la UI. El modelo es puro (sin E/S):
//!   cada directorio se lee a demanda al expandirlo ([`TreeModel::begin_load`],
//!   [`TreeModel::finish_load`]) y solo una vez ([`TreeModel::toggle`] +
//!   [`TreeModel::begin_load`] nunca releen un directorio ya cargado).
//!
//! El orden de las entradas lo decide el listado que el explorador entrega
//! (P2-11: `opm_fs::list_tree_children`, la política del árbol — carpetas
//! primero case-insensitive, `IGNORED_NAMES` fuera); el modelo no reordena.

use opm_fs::TreeEntry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Filtros de visibilidad del explorador, por ventana (§14.4, §14.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExplorerFilters {
    /// `Show All Files` (§14.4): sin filtro de tipos documentales.
    pub show_all: bool,
    /// `Show Hidden Files` (§14.5): muestra entradas que empiezan por `.`.
    pub show_hidden: bool,
}

/// Extensiones de los tipos documentales visibles por defecto (§14.4):
/// texto/plano, Markdown, YAML, JSON, shell, FASTA, imágenes (PNG, JPG/JPEG,
/// SVG, WebP, GIF) y configuración/tablas (CFG, INI, TSV, CSV).
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "yml", "yaml", "json", "sh", "bash", "fa", "fasta", "fna", "ffn",
    "faa", "png", "jpg", "jpeg", "svg", "webp", "gif", "cfg", "ini", "tsv", "csv",
];

/// ¿El nombre es un tipo documental visible por defecto (§14.4)?
///
/// Comparación por extensión (después del último `.`), insensible a
/// mayúsculas en el nombre del archivo (tolerancia de UI; el core §12.2 es
/// estricto para abrir documentos). Un nombre sin tallo (`.md`, un oculto
/// llamado exactamente «.md») no tiene extensión.
pub fn is_document_type(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) else {
        return false;
    };
    DOCUMENT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

/// ¿Se muestra `entry` con `filters`? (§14.4, §14.5)
///
/// Los directorios siempre se muestran (son la navegación del árbol).
pub fn entry_visible(entry: &TreeEntry, filters: ExplorerFilters) -> bool {
    if !filters.show_hidden && entry.name.starts_with('.') {
        return false;
    }
    entry.is_dir || filters.show_all || is_document_type(&entry.name)
}

/// Estado de carga de un directorio del árbol (§47.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeState {
    /// Sin leer (se leerá al expandirlo).
    Unloaded,
    /// Lectura en curso en una tarea aparte (§47.1: nunca bloquea la UI).
    Loading,
    /// Leído: entradas directas en el orden del listado perezoso (la
    /// política del árbol de `opm-fs`, P2-11).
    Loaded(Vec<TreeEntry>),
    /// Lectura fallida; se reintenta al volver a expandir.
    Failed(String),
}

/// Fila visible del árbol: una entrada con su nivel de indentación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    /// Nivel de anidación; la raíz del proyecto no se lista (sus hijos son 0).
    pub depth: usize,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// ¿Está expandido ahora mismo?
    pub expanded: bool,
    /// Solo los directorios reales se expanden: las carpetas enlazadas
    /// mediante symlink no se recorren (§14.7).
    pub expandable: bool,
}

/// Árbol perezoso del proyecto: qué directorios están expandidos/cargados y
/// qué filas se renderizan. Sin E/S: el llamador lee y llama a
/// [`TreeModel::finish_load`].
pub struct TreeModel {
    root: PathBuf,
    filters: ExplorerFilters,
    nodes: BTreeMap<PathBuf, NodeState>,
    expanded: BTreeSet<PathBuf>,
}

impl TreeModel {
    /// Modelo de `root` sin leer nada (§47.5: nada se carga hasta expandir o
    /// pedir la raíz).
    pub fn new(root: PathBuf) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(root.clone(), NodeState::Unloaded);
        Self {
            root,
            filters: ExplorerFilters::default(),
            nodes,
            expanded: BTreeSet::new(),
        }
    }

    /// Raíz canónica del árbol (la usa la carga en hilo para el listado
    /// perezoso con política, P2-11).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Recarga completa tras operaciones §14.8: cache vacía y sin expandir,
    /// conservando los filtros por ventana.
    pub fn reset(&mut self) {
        self.nodes.clear();
        self.nodes.insert(self.root.clone(), NodeState::Unloaded);
        self.expanded.clear();
    }

    pub fn filters(&self) -> ExplorerFilters {
        self.filters
    }

    /// Cambia los filtros (§14.4, §14.5); se aplican al renderizar, sin
    /// releer directorios.
    pub fn set_filters(&mut self, filters: ExplorerFilters) {
        self.filters = filters;
    }

    /// Estado de carga de `dir`; un directorio desconocido es [`NodeState::Unloaded`].
    pub fn state(&self, dir: &Path) -> &NodeState {
        static UNLOADED: NodeState = NodeState::Unloaded;
        self.nodes.get(dir).unwrap_or(&UNLOADED)
    }

    /// Pide comenzar la lectura de `dir` (§47.5). `true` si el llamador debe
    /// lanzar la tarea de lectura; `false` si ya está cargada o en curso (nunca
    /// se lee el mismo directorio dos veces). Un fallo previo se reintenta.
    pub fn begin_load(&mut self, dir: &Path) -> bool {
        let state = self
            .nodes
            .entry(dir.to_path_buf())
            .or_insert(NodeState::Unloaded);
        if matches!(state, NodeState::Unloaded | NodeState::Failed(_)) {
            *state = NodeState::Loading;
            true
        } else {
            false
        }
    }

    /// Aplica el resultado de leer `dir` (§47.5).
    pub fn finish_load(&mut self, dir: &Path, result: Result<Vec<TreeEntry>, String>) {
        let state = match result {
            Ok(entries) => NodeState::Loaded(entries),
            Err(message) => NodeState::Failed(message),
        };
        self.nodes.insert(dir.to_path_buf(), state);
    }

    /// Expande o colapsa `dir` y devuelve `true` si queda expandido.
    ///
    /// Solo los directorios reales (no symlink) se expanden (§14.7); una
    /// entrada que no es expandible es un no-op que devuelve `false`.
    pub fn toggle(&mut self, dir: &Path) -> bool {
        if !self.expandable(dir) {
            return false;
        }
        if self.expanded.remove(dir) {
            false
        } else {
            self.expanded.insert(dir.to_path_buf());
            true
        }
    }

    /// ¿Es `dir` un directorio real expandible (§14.7)? Se decide sobre las
    /// entradas ya cargadas de su padre; la raíz siempre es expandible.
    pub fn expandable(&self, dir: &Path) -> bool {
        if dir == self.root {
            return true;
        }
        let Some(parent) = dir.parent() else {
            return false;
        };
        match self.nodes.get(parent) {
            Some(NodeState::Loaded(entries)) => entries
                .iter()
                .find(|entry| entry.path == dir)
                .map(|entry| entry.is_dir && !entry.is_symlink)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Filas visibles, en preorden (padre → sus hijos → siguiente hermano),
    /// con los filtros §14.4/§14.5 aplicados y EL ORDEN del listado que
    /// cargó (P2-11: el modelo no reordena).
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.push_children(&self.root.clone(), 0, &mut rows);
        rows
    }

    fn push_children(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let NodeState::Loaded(entries) = self.state(dir) else {
            return;
        };
        for entry in entries.iter().filter(|e| entry_visible(e, self.filters)) {
            let expandable = entry.is_dir && !entry.is_symlink;
            rows.push(Row {
                path: entry.path.clone(),
                name: entry.name.clone(),
                depth,
                is_dir: entry.is_dir,
                is_symlink: entry.is_symlink,
                expanded: expandable && self.expanded.contains(&entry.path),
                expandable,
            });
            if expandable && self.expanded.contains(&entry.path) {
                self.push_children(&entry.path.clone(), depth + 1, rows);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    use std::fs;
    use std::os::unix::fs::symlink;

    fn temp_tree(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "opm-explorer-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(path: &Path) -> TreeEntry {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        TreeEntry {
            path: path.to_path_buf(),
            name,
            is_symlink: false,
            is_dir: false,
        }
    }

    fn dir_entry(path: &Path) -> TreeEntry {
        TreeEntry {
            is_dir: true,
            ..entry(path)
        }
    }

    fn symlink_entry(path: &Path, to_dir: bool) -> TreeEntry {
        TreeEntry {
            is_symlink: true,
            is_dir: to_dir,
            ..entry(path)
        }
    }

    /// Carga `entries` como contenido de `dir` y devuelve cuántas lecturas se
    /// pidieron de verdad (§47.5: solo las necesarias).
    fn load(model: &mut TreeModel, dir: &Path, entries: Vec<TreeEntry>) -> usize {
        let reads = if model.begin_load(dir) { 1 } else { 0 };
        model.finish_load(dir, Ok(entries));
        reads
    }

    #[test]
    fn tipos_documentales_14_4() {
        for name in [
            "notas.txt",
            "doc.md",
            "doc.markdown",
            "pipeline.yml",
            "state.json",
            "run.sh",
            "run.bash",
            "genome.fa",
            "genome.fasta",
            "contigs.fna",
            "genes.ffn",
            "proteins.faa",
            "foto.png",
            "foto.JPG",
            "foto.jpeg",
            "foto.svg",
            "foto.webp",
            "foto.gif",
            "config.cfg",
            "config.ini",
            "tabla.tsv",
            "tabla.csv",
        ] {
            assert!(
                is_document_type(name),
                "{name} debería ser visible por defecto (§14.4)"
            );
        }
        for name in [
            "video.mp4",
            "refs.bib",
            "config.toml",
            "sin-extension",
            "doc.md.bak",
            ".md",
        ] {
            assert!(
                !is_document_type(name),
                "{name} no es tipo documental (§14.4)"
            );
        }
    }

    #[test]
    fn por_defecto_solo_tipos_documentales_14_4() {
        let root = temp_tree("tipos");
        let mut model = TreeModel::new(root.clone());
        assert_eq!(
            load(
                &mut model,
                &root,
                vec![
                    dir_entry(&root.join("cap1")),
                    entry(&root.join("doc.md")),
                    entry(&root.join("notas.txt")),
                    entry(&root.join("pipeline.yml")),
                    entry(&root.join("run.sh")),
                    entry(&root.join("genome.fa")),
                    entry(&root.join("datos.csv")),
                    entry(&root.join("foto.png")),
                    entry(&root.join("refs.bib")),
                    entry(&root.join("config.toml")),
                    entry(&root.join("video.mp4")),
                ]
            ),
            1
        );

        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            nombres,
            [
                "cap1",
                "doc.md",
                "notas.txt",
                "pipeline.yml",
                "run.sh",
                "genome.fa",
                "datos.csv",
                "foto.png"
            ]
        );

        // Show All Files (§14.4) muestra todo.
        model.set_filters(ExplorerFilters {
            show_all: true,
            ..Default::default()
        });
        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(nombres.len(), 11);
        assert!(nombres.contains(&"video.mp4") && nombres.contains(&"datos.csv"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ocultos_ocultos_por_defecto_14_5() {
        let root = temp_tree("ocultos");
        let mut model = TreeModel::new(root.clone());
        load(
            &mut model,
            &root,
            vec![
                entry(&root.join("doc.md")),
                entry(&root.join(".oculto.md")),
                dir_entry(&root.join(".carpeta")),
            ],
        );
        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(nombres, ["doc.md"]);

        // Show Hidden Files (§14.5) revela los ocultos…
        model.set_filters(ExplorerFilters {
            show_hidden: true,
            ..Default::default()
        });
        load(
            &mut model,
            &root,
            vec![
                entry(&root.join("doc.md")),
                entry(&root.join(".oculto.md")),
                entry(&root.join(".oculto.mp4")),
            ],
        );
        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(nombres, ["doc.md", ".oculto.md"]);

        // …y es independiente de Show All Files: `.oculto.mp4` solo se ve
        // con ambos.
        model.set_filters(ExplorerFilters {
            show_all: true,
            show_hidden: true,
        });
        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(nombres, ["doc.md", ".oculto.md", ".oculto.mp4"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn los_ignorados_por_gitignore_siguen_visibles_14_6() {
        // §14.6: la exclusión de `.gitignore` aplica a búsqueda/indexado
        // (ISSUE-025/028), nunca al explorador.
        let root = temp_tree("gitignore");
        fs::write(root.join(".gitignore"), "salida.md\ncache/\n").unwrap();
        let mut model = TreeModel::new(root.clone());
        load(
            &mut model,
            &root,
            vec![
                entry(&root.join("salida.md")),
                dir_entry(&root.join("cache")),
            ],
        );
        load(
            &mut model,
            &root.join("cache"),
            vec![entry(&root.join("cache/dentro.md"))],
        );
        assert!(model.toggle(&root.join("cache")));

        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(nombres, ["salida.md", "cache", "dentro.md"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn symlinks_visibles_sin_recursion_14_7() {
        let root = temp_tree("symlinks");
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, root.join("enlazada")).unwrap();
        symlink(root.join("doc.md"), root.join("enlace.md")).unwrap();
        fs::write(root.join("doc.md"), "# doc\n").unwrap();

        let mut model = TreeModel::new(root.clone());
        load(
            &mut model,
            &root,
            vec![
                symlink_entry(&root.join("enlazada"), true),
                symlink_entry(&root.join("enlace.md"), false),
                dir_entry(&real),
                entry(&root.join("doc.md")),
            ],
        );

        let rows = model.rows();
        let por_nombre = |name: &str| rows.iter().find(|r| r.name == name).unwrap().clone();
        assert!(por_nombre("enlace.md").is_symlink && !por_nombre("enlace.md").expandable);
        assert!(por_nombre("enlazada").is_symlink && por_nombre("enlazada").is_dir);
        // La carpeta enlazada se muestra pero no se recorre nunca (§14.7).
        assert!(!por_nombre("enlazada").expandable);
        assert!(!model.toggle(&root.join("enlazada")));
        assert!(por_nombre("real").expandable);

        // Un symlink roto se lista como entrada no-directorio.
        assert!(!symlink_entry(&root.join("roto"), false).is_dir);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn el_modelo_preserva_el_orden_del_listado_de_politica() {
        let root = temp_tree("orden");
        for name in ["b.md", "a-carpeta", "c.md", "d-carpeta"] {
            let path = root.join(name);
            if name.ends_with("carpeta") {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(&path, "# doc\n").unwrap();
            }
        }
        let mut model = TreeModel::new(root.clone());
        let en_disco = opm_fs::list_tree_children(&root, &root).unwrap();
        load(&mut model, &root, en_disco.clone());

        // El modelo NO reordena: respeta el orden del listado perezoso que
        // en producción entrega `opm_fs::list_tree_children` (P2-11: la
        // política del árbol — carpetas antes que ficheros, case-insensitive).
        let rows = model.rows();
        let nombres: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        let esperado: Vec<&str> = en_disco.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(nombres, ["a-carpeta", "d-carpeta", "b.md", "c.md"]);
        assert_eq!(nombres, esperado);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn carga_incremental_por_directorio_47_5() {
        // Miles de archivos: el modelo no lee nada por sí solo y solo lee el
        // directorio que el usuario expande (§47.5).
        let root = temp_tree("incremental");
        let mut model = TreeModel::new(root.clone());
        assert_eq!(model.state(&root), &NodeState::Unloaded);
        assert!(model.rows().is_empty(), "sin lecturas no hay filas");

        // Raíz con 200 subdirectorios (sintético, sin E/S): una sola lectura.
        let hijos: Vec<TreeEntry> = (0..200)
            .map(|i| dir_entry(&root.join(format!("sub{i:03}"))))
            .collect();
        let mut lecturas = load(&mut model, &root, hijos);
        assert_eq!(lecturas, 1);
        assert_eq!(model.rows().len(), 200);
        for i in 0..200 {
            let sub = root.join(format!("sub{i:03}"));
            assert_eq!(
                model.state(&sub),
                &NodeState::Unloaded,
                "nadie debe precargar sub{i:03}"
            );
        }

        // Expandir un hijo lee SOLO ese hijo; los demás siguen sin leer.
        let sub = root.join("sub042");
        assert!(model.toggle(&sub));
        lecturas += load(&mut model, &sub, vec![entry(&sub.join("nota.md"))]);
        assert_eq!(lecturas, 2);
        assert_eq!(model.rows().len(), 201);
        assert_eq!(model.state(&root.join("sub000")), &NodeState::Unloaded);
        assert_eq!(model.state(&root.join("sub199")), &NodeState::Unloaded);

        // Colapsar y volver a expandir no relea (§47.5: sin trabajo en vano).
        assert!(!model.toggle(&sub));
        assert_eq!(model.rows().len(), 200);
        assert!(model.toggle(&sub));
        assert_eq!(model.rows().len(), 201);
        assert!(!model.begin_load(&sub), "ya cargado: no se relee");
        assert!(!model.begin_load(&sub), "ni siquiera en paralelo");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn carga_en_curso_no_se_duplica() {
        let root = temp_tree("loading");
        let mut model = TreeModel::new(root.clone());
        assert!(model.begin_load(&root));
        assert_eq!(model.state(&root), &NodeState::Loading);
        // Una segunda petición (doble click, redraw) no lanza otra lectura.
        assert!(!model.begin_load(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fallo_de_lectura_se_reintenta() {
        let root = temp_tree("fallo");
        let mut model = TreeModel::new(root.clone());
        assert!(model.begin_load(&root));
        model.finish_load(&root, Err("permiso denegado".to_owned()));
        assert_eq!(
            model.state(&root),
            &NodeState::Failed("permiso denegado".to_owned())
        );
        assert!(model.rows().is_empty());
        // Reintento al volver a pedirlo (por ejemplo al reexpandir).
        assert!(model.begin_load(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn anidacion_expandida_renderiza_con_profundidad() {
        let root = temp_tree("anidacion");
        let mut model = TreeModel::new(root.clone());
        load(&mut model, &root, vec![dir_entry(&root.join("a"))]);
        load(
            &mut model,
            &root.join("a"),
            vec![dir_entry(&root.join("a/b")), entry(&root.join("a/x.md"))],
        );
        load(
            &mut model,
            &root.join("a/b"),
            vec![entry(&root.join("a/b/y.md"))],
        );

        // Solo lo expandido se renderiza.
        assert_eq!(model.rows().len(), 1);
        assert!(model.toggle(&root.join("a")));
        assert!(model.toggle(&root.join("a/b")));
        let rows = model.rows();
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.depth))
                .collect::<Vec<_>>(),
            [("a", 0), ("b", 1), ("y.md", 2), ("x.md", 1)]
        );
        let _ = fs::remove_dir_all(&root);
    }
}
