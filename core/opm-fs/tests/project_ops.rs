//! Behavior ports of `backend/tests/test_project_tree.py` and the relevant
//! parts of `backend/tests/test_file_upload_api.py` into Rust (no HTTP
//! wiring): scaffold, step folders, guarded file ops and their error kinds,
//! batch-import preflight/rollback, tree depth cap + ordering and preview
//! classification.

use opm_fs::{
    build_tree, create_project_path, delete_project_path, ensure_project_scaffold,
    ensure_step_folders, import_files, list_directories, list_filesystem, list_tree_children,
    move_project_path, preview_file, rename_project_path, validate_project_folder,
    write_project_file, FilePreview, ImportSource, OpsError, PathError, TreeKind, TreeNode,
    IGNORED_NAMES, PROJECT_CONFIG_FILENAME,
};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Directorio temporal único (patrón de `test_dir.rs`), borrado al salir.
struct TestDir(PathBuf);

impl TestDir {
    fn new(prefix: &str) -> Self {
        for _ in 0..100 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let cuenta = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nanos}-{cuenta}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("no se pudo crear el temporal {path:?}: {err}"),
            }
        }
        panic!("sin directorio temporal libre");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Proyecto scaffoldeado + su raíz canónica (igual que la que devuelven las
/// operaciones resueltas).
fn project(prefix: &str) -> (TestDir, PathBuf) {
    let dir = TestDir::new(prefix);
    ensure_project_scaffold(dir.path(), "demo").unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

/// Ruta relativa con `/` respecto de la raíz (como la aserción
/// `relative_to(...).as_posix()` de Python).
fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or_else(|_| panic!("{path:?} no está bajo {root:?}"))
        .to_string_lossy()
        .replace('\\', "/")
}

/// Symlink portable; `false` = no disponible en esta máquina (el test no
/// aplica, como el `_symlink_or_skip` de Python).
fn symlink_or_skip(original: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link).is_ok()
    }
    #[cfg(windows)]
    {
        let result = if original.is_dir() {
            std::os::windows::fs::symlink_dir(original, link)
        } else {
            std::os::windows::fs::symlink_file(original, link)
        };
        result.is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (original, link);
        false
    }
}

fn find<'a>(node: &'a TreeNode, path: &str) -> &'a TreeNode {
    let mut current = node;
    for segment in path.split('/') {
        current = current
            .children
            .iter()
            .find(|child| child.name == segment)
            .unwrap_or_else(|| panic!("falta {segment} en {path}"));
    }
    current
}

// ---------------------------------------------------------------- scaffold --

#[test]
fn scaffold_creates_workspace_dirs_and_config() {
    let dir = TestDir::new("scaffold");
    ensure_project_scaffold(dir.path(), "My Pipeline").unwrap();

    assert!(dir.path().join("Input").is_dir());
    assert!(dir.path().join("steps").is_dir());
    assert!(dir.path().join("outputs").is_dir());
    assert!(dir.path().join("config.txt").is_file());
    assert!(dir.path().join(PROJECT_CONFIG_FILENAME).is_file());

    let config = fs::read_to_string(dir.path().join(PROJECT_CONFIG_FILENAME)).unwrap();
    assert!(config.contains("\"name\": \"My Pipeline\""));
    assert!(config.contains("\"version\": 1"));
    assert!(config.contains("\"created_at\": \""));
    assert!(config.contains("\"input\": \"Input\""));
    assert!(config.contains("\"steps\": \"steps\""));
    assert!(config.contains("\"outputs\": \"outputs\""));

    let config_txt = fs::read_to_string(dir.path().join("config.txt")).unwrap();
    assert!(config_txt.starts_with("Open Pipeline Manager project\n"));

    // create-if-absent: un segundo scaffold no pisa el contenido del usuario.
    fs::write(dir.path().join("config.txt"), "sentinel").unwrap();
    ensure_project_scaffold(dir.path(), "Other").unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("config.txt")).unwrap(),
        "sentinel"
    );
    let config = fs::read_to_string(dir.path().join(PROJECT_CONFIG_FILENAME)).unwrap();
    assert!(config.contains("\"name\": \"My Pipeline\""));
}

#[test]
fn validate_requires_project_folder_and_config() {
    let dir = TestDir::new("validate");
    // Carpeta sin project.config.json → NotAProject (Python ValueError).
    let err = validate_project_folder(dir.path()).unwrap_err();
    assert!(matches!(err, OpsError::NotAProject), "{err:?}");
    assert!(err.to_string().contains(PROJECT_CONFIG_FILENAME));

    ensure_project_scaffold(dir.path(), "demo").unwrap();
    validate_project_folder(dir.path()).unwrap();

    // Inexistente → NotFound (Python FileNotFoundError).
    let err = validate_project_folder(&dir.path().join("ghost")).unwrap_err();
    assert!(matches!(err, OpsError::NotFound { .. }), "{err:?}");
}

#[test]
fn step_folders_created_from_ids() {
    let (dir, root) = project("step-folders");
    ensure_step_folders(&root, "prepare-data").unwrap();
    for dirname in ["input", "work", "output"] {
        assert!(root.join("steps/prepare-data").join(dirname).is_dir());
    }
    // safe_segment aplicado al id (como en Python).
    ensure_step_folders(&root, "My Step/2").unwrap();
    assert!(root.join("steps/My_Step_2/work").is_dir());
    drop(dir);
}

/// F2 (escenario 1): `steps/<id>` enlazado hacia un directorio de fuera —
/// el workspace jamás se crea fuera del proyecto y el error es visible.
#[test]
fn step_folders_refuse_symlinked_workspace_pointing_outside() {
    let (dir, root) = project("step-folders-outside");
    let outside = TestDir::new("step-folders-outside-sentinel");
    fs::write(outside.path().join("sentinel.txt"), "keep").unwrap();
    if !symlink_or_skip(outside.path(), &root.join("steps/a")) {
        drop(dir);
        return;
    }

    let err = ensure_step_folders(&root, "a").unwrap_err();
    assert!(
        err.to_string().contains("outside the current project"),
        "{err}"
    );
    // Cero escrituras fuera: ni input/work/output ni nada más.
    assert!(!outside.path().join("input").exists());
    assert!(!outside.path().join("work").exists());
    assert!(!outside.path().join("output").exists());
    assert_eq!(
        fs::read_to_string(outside.path().join("sentinel.txt")).unwrap(),
        "keep"
    );
    drop(dir);
}

/// F2 (regla b): un enlace DENTRO del proyecto también se rechaza — el
/// prefijo gestionado `steps/<id>/...` nunca contiene symlinks.
#[test]
fn step_folders_refuse_symlinked_components_inside_the_project() {
    let (dir, root) = project("step-folders-inside");
    fs::create_dir(root.join("real")).unwrap();
    if !symlink_or_skip(&root.join("real"), &root.join("steps/b")) {
        drop(dir);
        return;
    }

    let err = ensure_step_folders(&root, "b").unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");
    assert!(!root.join("real/input").exists());
    assert!(!root.join("real/work").exists());
    assert!(!root.join("real/output").exists());
    drop(dir);
}

/// F2: la propia raíz de trabajo `steps` enlazada hacia fuera se rechaza
/// antes de crear (o seguir) nada.
#[test]
fn step_folders_refuse_symlinked_steps_root() {
    let (dir, root) = project("step-folders-root");
    let outside = TestDir::new("step-folders-root-outside");
    fs::remove_dir(root.join("steps")).unwrap();
    if !symlink_or_skip(outside.path(), &root.join("steps")) {
        drop(dir);
        return;
    }

    let err = ensure_step_folders(&root, "a").unwrap_err();
    assert!(
        err.to_string().contains("outside the current project"),
        "{err}"
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    drop(dir);
}

// ------------------------------------------------------------- file ops ----

#[test]
fn create_rename_move_delete_flow() {
    let (dir, root) = project("flow");
    let created = create_project_path(&root, "Input/sample.txt", "hello", false).unwrap();
    assert_eq!(created, root.join("Input").join("sample.txt"));
    assert_eq!(fs::read_to_string(&created).unwrap(), "hello");

    let renamed = rename_project_path(&root, "Input/sample.txt", "renamed.txt").unwrap();
    assert_eq!(rel(&root, &renamed), "Input/renamed.txt");

    let moved = move_project_path(&root, "Input/renamed.txt", "outputs").unwrap();
    assert_eq!(rel(&root, &moved), "outputs/renamed.txt");

    delete_project_path(&root, "outputs/renamed.txt").unwrap();
    assert!(!moved.exists());
    drop(dir);
}

#[test]
fn create_rejects_existing_path_without_touching_it() {
    let (dir, root) = project("create-exists");
    create_project_path(&root, "Input/sample.txt", "hello", false).unwrap();

    let err = create_project_path(&root, "Input/sample.txt", "again", false).unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert_eq!(
        fs::read_to_string(root.join("Input/sample.txt")).unwrap(),
        "hello"
    );

    // Una carpeta existente también es conflicto (Python FileExistsError).
    let err = create_project_path(&root, "steps", "", true).unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    drop(dir);
}

#[test]
fn write_overwrites_files_only() {
    let (dir, root) = project("write");
    create_project_path(&root, "Input/config.yml", "old", false).unwrap();

    let written = write_project_file(&root, "Input/config.yml", "new").unwrap();
    assert_eq!(rel(&root, &written), "Input/config.yml");
    assert_eq!(
        fs::read_to_string(root.join("Input/config.yml")).unwrap(),
        "new"
    );

    // Crea los padres que falten (Python mkdir parents=True).
    write_project_file(&root, "outputs/deep/dir/file.txt", "x").unwrap();
    assert!(root.join("outputs/deep/dir/file.txt").is_file());

    // Un directorio no se puede sobrescribir (Python ValueError).
    let err = write_project_path_err(&root);
    assert!(matches!(err, OpsError::TargetNotAFile), "{err:?}");
    drop(dir);
}

fn write_project_path_err(root: &Path) -> OpsError {
    write_project_file(root, "Input", "x").unwrap_err()
}

#[test]
fn traversal_and_absolute_paths_refused_on_any_platform() {
    let (dir, root) = project("escape");
    let err = create_project_path(&root, "../evil.txt", "x", false).unwrap_err();
    assert!(
        matches!(err, OpsError::Path(PathError::OutsideProject { .. })),
        "{err:?}"
    );

    let err = create_project_path(&root, "/abs-evil.txt", "x", false).unwrap_err();
    assert!(
        matches!(err, OpsError::Path(PathError::Absolute { .. })),
        "{err:?}"
    );

    // Formas de Windows rechazadas también en POSIX (pin de test_validation).
    for raw in ["C:\\abs-evil.txt", "D:/abs-evil.txt", "\\abs-evil.txt"] {
        let err = create_project_path(&root, raw, "x", false).unwrap_err();
        assert!(
            matches!(err, OpsError::Path(PathError::Absolute { .. })),
            "{raw} -> {err:?}"
        );
    }

    // El directorio destino ".." de una subida se rechaza por salirse del
    // proyecto (test_upload_file_rejects_path_escape).
    let err = import_files(&root, "..", &[]).unwrap_err();
    assert!(
        matches!(err, OpsError::Path(PathError::OutsideProject { .. })),
        "{err:?}"
    );
    assert!(!root.parent().unwrap().join("evil.txt").exists());
    drop(dir);
}

#[test]
fn rename_rules_root_single_segment_conflict() {
    let (dir, root) = project("rename");
    create_project_path(&root, "Input/a.txt", "A", false).unwrap();
    create_project_path(&root, "Input/b.txt", "B", false).unwrap();

    let err = rename_project_path(&root, ".", "other").unwrap_err();
    assert!(
        matches!(
            err,
            OpsError::RootForbidden {
                operation: "rename"
            }
        ),
        "{err:?}"
    );

    for bad in ["sub/name.txt", "sub\\name.txt", "   ", ""] {
        let err = rename_project_path(&root, "Input/a.txt", bad).unwrap_err();
        assert!(matches!(err, OpsError::NotSingleSegment), "{bad} -> {err:?}");
    }

    // Conflicto → AlreadyExists (409) y nada cambia.
    let err = rename_project_path(&root, "Input/a.txt", "b.txt").unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(root.join("Input/a.txt")).unwrap(), "A");
    assert_eq!(fs::read_to_string(root.join("Input/b.txt")).unwrap(), "B");
    drop(dir);
}

#[test]
fn move_rules_dir_target_no_recursion_conflict() {
    let (dir, root) = project("move");
    create_project_path(&root, "Input/x.txt", "X", false).unwrap();
    create_project_path(&root, "outputs/x.txt", "O", false).unwrap();

    let err = move_project_path(&root, ".", "outputs").unwrap_err();
    assert!(
        matches!(
            err,
            OpsError::RootForbidden {
                operation: "move"
            }
        ),
        "{err:?}"
    );

    // Destino que no es directorio (inexistente o fichero).
    let err = move_project_path(&root, "Input/x.txt", "nope-dir").unwrap_err();
    assert!(matches!(err, OpsError::TargetNotDirectory), "{err:?}");
    let err = move_project_path(&root, "Input/x.txt", "config.txt").unwrap_err();
    assert!(matches!(err, OpsError::TargetNotDirectory), "{err:?}");

    // Sin recursión: dentro de sí mismo o sobre sí mismo.
    fs::create_dir(root.join("steps/inner")).unwrap();
    let err = move_project_path(&root, "steps", "steps/inner").unwrap_err();
    assert!(matches!(err, OpsError::DirectoryIntoItself), "{err:?}");
    let err = move_project_path(&root, "steps", "steps").unwrap_err();
    assert!(matches!(err, OpsError::DirectoryIntoItself), "{err:?}");

    // Conflicto → AlreadyExists (409) y el origen sigue ahí.
    let err = move_project_path(&root, "Input/x.txt", "outputs").unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(root.join("Input/x.txt")).unwrap(), "X");
    drop(dir);
}

#[test]
fn delete_rules_root_ignored_names_missing_recursive() {
    let (dir, root) = project("delete");
    for name in IGNORED_NAMES {
        fs::create_dir(root.join(name)).unwrap();
    }

    let err = delete_project_path(&root, ".").unwrap_err();
    assert!(
        matches!(
            err,
            OpsError::RootForbidden {
                operation: "delete"
            }
        ),
        "{err:?}"
    );

    // Ninguna carpeta gestionada/ignorada se borra (Python ValueError).
    for name in IGNORED_NAMES {
        let err = delete_project_path(&root, name).unwrap_err();
        assert!(matches!(err, OpsError::IgnoredName { .. }), "{name}: {err:?}");
        assert!(root.join(name).exists(), "{name} debe seguir intacta");
    }

    let err = delete_project_path(&root, "Input/ghost.txt").unwrap_err();
    assert!(matches!(err, OpsError::NotFound { .. }), "{err:?}");

    // Borrado recursivo de directorios.
    fs::create_dir_all(root.join("outputs/outer/inner")).unwrap();
    fs::write(root.join("outputs/outer/inner/f.txt"), "x").unwrap();
    delete_project_path(&root, "outputs/outer").unwrap();
    assert!(!root.join("outputs/outer").exists());
    drop(dir);
}

// ------------------------------------------------------------ import batch --

fn src_file(dir: &TestDir, name: &str, content: &str) -> ImportSource {
    fs::write(dir.path().join(name), content).unwrap();
    ImportSource {
        source: dir.path().join(name),
        name: name.to_owned(),
    }
}

#[test]
fn import_copies_batch_in_order() {
    let (dir, root) = project("import-ok");
    let src = TestDir::new("import-ok-src");
    let files = [
        src_file(&src, "a.txt", "first"),
        src_file(&src, "b.txt", "second"),
    ];

    let created = import_files(&root, "Input", &files).unwrap();
    assert_eq!(
        created,
        vec![
            root.join("Input").join("a.txt"),
            root.join("Input").join("b.txt")
        ]
    );
    assert_eq!(fs::read_to_string(root.join("Input/a.txt")).unwrap(), "first");
    assert_eq!(
        fs::read_to_string(root.join("Input/b.txt")).unwrap(),
        "second"
    );
    drop(dir);
}

#[test]
fn import_preflights_existing_conflict_before_writing() {
    // Port de test_multi_file_upload_preflights_conflicts_before_writing.
    let (dir, root) = project("import-preflight");
    let src = TestDir::new("import-preflight-src");
    fs::write(root.join("Input/existing.txt"), "keep").unwrap();

    let err = import_files(
        &root,
        "Input",
        &[
            src_file(&src, "first.txt", "first"),
            ImportSource {
                source: src_file(&src, "existing.txt", "new").source,
                name: "existing.txt".into(),
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert!(!root.join("Input/first.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("Input/existing.txt")).unwrap(),
        "keep"
    );
    drop(dir);
}

#[test]
fn import_rejects_duplicate_destinations_including_whitespace() {
    // Port de test_multi_file_upload_rejects_duplicate_destinations_before_writing.
    let (dir, root) = project("import-dup");
    let src = TestDir::new("import-dup-src");

    let err = import_files(
        &root,
        "Input",
        &[
            src_file(&src, "duplicate.txt", "first"),
            ImportSource {
                source: root.join("Input/duplicate.txt"), // no llega a leerse
                name: " duplicate.txt ".into(),
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert!(!root.join("Input/duplicate.txt").exists());
    drop(dir);
}

#[test]
fn import_rejects_case_equivalent_destinations_on_insensitive_branch() {
    // P2 (paridad): en un filesystem de destino case-insensitive
    // `report.txt` vs `REPORT.txt` pasan la comparación exacta y chocarían
    // en la reserva con un error genérico — el preflight los rechaza como
    // duplicados ANTES de escribir nada. La rama se fuerza con el gancho
    // (por hilo) para poder clavarla también en un host case-sensitive.
    let (dir, root) = project("import-case-dup");
    let src = TestDir::new("import-case-dup-src");

    opm_fs::force_case_insensitive_destinations(Some(true));
    let err = import_files(
        &root,
        "Input",
        &[
            src_file(&src, "report.txt", "first"),
            src_file(&src, "REPORT.txt", "second"),
        ],
    )
    .unwrap_err();
    opm_fs::force_case_insensitive_destinations(None);

    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert!(!root.join("Input/report.txt").exists());
    assert!(!root.join("Input/REPORT.txt").exists());
    drop(dir);
}

#[test]
fn import_accepts_case_distinct_destinations_on_sensitive_branch() {
    // P2: en la rama case-sensitive (Linux por defecto) `report.txt` y
    // `REPORT.txt` son destinos legítimamente distintos: el preflight no
    // los rechaza y la reserva exclusiva crea los dos.
    let (dir, root) = project("import-case-ok");
    let src = TestDir::new("import-case-ok-src");

    // Skip donde el FS real es case-insensitive (APFS/NTFS): crear
    // report.txt y REPORT.txt colisiona físicamente. La rama sensible del
    // folding queda pinzada por el test unitario de `destinations_conflict`.
    let probe_a = src.path().join("case-probe.txt");
    let probe_b = src.path().join("CASE-PROBE.TXT");
    if fs::write(&probe_a, "a").is_err() {
        return;
    }
    if fs::write(&probe_b, "b").is_err()
        || fs::read_to_string(&probe_a).unwrap_or_default() != "a"
    {
        eprintln!("skip: filesystem case-insensitive");
        return;
    }

    opm_fs::force_case_insensitive_destinations(Some(false));
    let created = import_files(
        &root,
        "Input",
        &[
            src_file(&src, "report.txt", "first"),
            src_file(&src, "REPORT.txt", "second"),
        ],
    );
    opm_fs::force_case_insensitive_destinations(None);

    assert_eq!(
        created.unwrap(),
        vec![
            root.join("Input/report.txt"),
            root.join("Input/REPORT.txt"),
        ]
    );
    assert_eq!(
        fs::read_to_string(root.join("Input/report.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(root.join("Input/REPORT.txt")).unwrap(),
        "second"
    );
    drop(dir);
}

#[test]
fn import_rejects_unreadable_later_source_before_writing() {
    // Port de test_multi_file_upload_rejects_invalid_later_content_before_writing:
    // el contenido de un destino posterior falla → nada se crea.
    let (dir, root) = project("import-badcontent");
    let src = TestDir::new("import-badcontent-src");

    let err = import_files(
        &root,
        "Input",
        &[
            src_file(&src, "first.txt", "first"),
            ImportSource {
                source: src.path().join("missing.txt"),
                name: "second.txt".into(),
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Io { .. }), "{err:?}");
    assert!(!root.join("Input/first.txt").exists());
    assert!(!root.join("Input/second.txt").exists());
    drop(dir);
}

#[test]
fn import_rolls_back_created_files_when_later_reservation_fails() {
    // Port de test_batch_upload_rolls_back_when_a_later_reservation_conflicts:
    // el primer destino se crea y el segundo falla en la reserva exclusiva
    // (nombre más largo que NAME_MAX: el preflight lo ve como inexistente y
    // la creación exclusiva lo rechaza) → solo se borra lo creado aquí.
    let (dir, root) = project("import-rollback");
    let src = TestDir::new("import-rollback-src");
    let long_name = format!("{}.txt", "x".repeat(300));

    let err = import_files(
        &root,
        "Input",
        &[
            src_file(&src, "first.txt", "first"),
            src_file(&src, "too-long-source.txt", "boom"),
        ]
        .map(|mut file| {
            if file.name == "too-long-source.txt" {
                file.name = long_name.clone();
            }
            file
        })
        .as_slice(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Io { .. }), "{err:?}");
    assert!(!root.join("Input/first.txt").exists());
    drop(dir);
}

#[test]
fn import_rejects_bad_names_and_missing_target_dir() {
    let (dir, root) = project("import-names");
    let src = TestDir::new("import-names-src");

    let err = import_files(
        &root,
        "nope-dir",
        &[src_file(&src, "a.txt", "x")],
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::TargetNotDirectory), "{err:?}");

    let err = import_files(
        &root,
        "Input",
        &[ImportSource {
            source: src.path().join("a.txt"),
            name: String::from("sub/f.txt"),
        }],
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::NotSingleSegment), "{err:?}");
    assert!(!root.join("Input/sub").exists());
    drop(dir);
}

#[cfg(unix)]
#[test]
fn import_refuses_symlink_targets_and_preserves_their_data() {
    let (dir, root) = project("import-symlink");
    let src = TestDir::new("import-symlink-src");
    fs::write(src.path().join("payload.txt"), "new").unwrap();
    let file = || ImportSource {
        source: src.path().join("payload.txt"),
        name: "linked.txt".into(),
    };

    // Symlink colgado: existe como entrada de directorio y no se reemplaza.
    std::os::unix::fs::symlink(root.join("missing-target.txt"), root.join("Input/dangling.txt"))
        .unwrap();
    let dangling = ImportSource {
        source: src.path().join("payload.txt"),
        name: "dangling.txt".into(),
    };
    let err = import_files(&root, "Input", &[dangling]).unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert!(
        fs::symlink_metadata(root.join("Input/dangling.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    // Symlink a un fichero exterior: no se sustituye ni se modifica el destino.
    let outside = TestDir::new("import-symlink-outside");
    fs::write(outside.path().join("outside.txt"), "original").unwrap();
    std::os::unix::fs::symlink(outside.path().join("outside.txt"), root.join("Input/linked.txt"))
        .unwrap();
    let err = import_files(&root, "Input", &[file()]).unwrap_err();
    assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
    assert_eq!(
        fs::read_to_string(outside.path().join("outside.txt")).unwrap(),
        "original"
    );
    drop(dir);
}

#[cfg(unix)]
#[test]
fn ops_reject_symlinks_that_escape_the_project() {
    let (dir, root) = project("ops-escape");
    let outside = TestDir::new("ops-escape-outside");
    fs::write(outside.path().join("target.txt"), "t").unwrap();
    assert!(symlink_or_skip(
        &outside.path().join("target.txt"),
        &root.join("Input/link.txt")
    ));

    // El borrado resuelve al destino: fuera del proyecto → rechazado.
    let err = delete_project_path(&root, "Input/link.txt").unwrap_err();
    assert!(
        matches!(err, OpsError::Path(PathError::OutsideProject { .. })),
        "{err:?}"
    );
    assert!(outside.path().join("target.txt").exists());

    // La creación sobre el enlace también se resuelve fuera y se rechaza.
    let err = create_project_path(&root, "Input/link.txt", "x", false).unwrap_err();
    assert!(
        matches!(err, OpsError::Path(PathError::OutsideProject { .. })),
        "{err:?}"
    );
    drop(dir);
}

// ------------------------------------------------------------------- tree ---

#[test]
fn tree_orders_dirs_first_case_insensitive_and_skips_ignored() {
    let dir = TestDir::new("tree-order");
    let root = fs::canonicalize(dir.path()).unwrap();
    for name in ["Bravo", "alpha"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    for name in ["Zeta.txt", "beta.txt"] {
        fs::write(root.join(name), "x").unwrap();
    }
    for name in [".git", "node_modules", "dist"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("dist/ignored.txt"), "x").unwrap();

    let tree = build_tree(&root).unwrap();
    assert_eq!(tree.path, ".");
    assert_eq!(tree.name, root.file_name().unwrap().to_string_lossy());
    assert!(matches!(tree.kind, TreeKind::Directory));

    let names: Vec<&str> = tree.children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(names, ["alpha", "Bravo", "beta.txt", "Zeta.txt"]);
    for ignored in [".git", "node_modules", "dist"] {
        assert!(!names.contains(&ignored), "{ignored} no debe listarse");
    }
}

#[test]
fn tree_caps_depth_at_five() {
    let dir = TestDir::new("tree-depth");
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("a/b/c/d/e/f")).unwrap();
    fs::write(root.join("a/b/c/d/file.txt"), "x").unwrap();
    fs::write(root.join("a/b/c/d/e/file.txt"), "x").unwrap();

    let tree = build_tree(&root).unwrap();
    // Nodos hasta la profundidad 5: a(1) b(2) c(3) d(4) e(5).
    let e = find(&tree, "a/b/c/d/e");
    assert!(matches!(e.kind, TreeKind::Directory));
    assert!(
        e.children.is_empty(),
        "más allá de la profundidad 5 no se enumeran hijos"
    );
    let file = find(&tree, "a/b/c/d/file.txt");
    assert!(matches!(file.kind, TreeKind::File));
    assert!(file.children.is_empty());
}

/// P2-11: el listado perezoso de un nivel respeta LA MISMA política que
/// `build_tree` (fuente única): orden carpetas-primero case-insensitive,
/// `IGNORED_NAMES` fuera y tope de profundidad 5.
#[test]
fn list_tree_children_shares_the_build_tree_policy() {
    let dir = TestDir::new("lazy-tree");
    let root = fs::canonicalize(dir.path()).unwrap();
    for name in ["Bravo", "alpha"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    for name in ["Zeta.txt", "beta.txt"] {
        fs::write(root.join(name), "x").unwrap();
    }
    for name in [".git", "node_modules", "dist"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::create_dir_all(root.join("a/b/c/d/e/f")).unwrap();
    fs::write(root.join("a/b/c/d/file.txt"), "x").unwrap();
    fs::write(root.join("a/b/c/d/e/file.txt"), "x").unwrap();

    // Raíz: mismo orden que `build_tree`, sin nombres ignorados.
    let root_children = list_tree_children(&root, &root).unwrap();
    let names: Vec<&str> = root_children
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["a", "alpha", "Bravo", "beta.txt", "Zeta.txt"]);
    for ignored in [".git", "node_modules", "dist"] {
        assert!(!names.contains(&ignored), "{ignored} no debe listarse");
    }
    assert!(root_children.iter().find(|e| e.name == "alpha").unwrap().is_dir);
    assert!(!root_children.iter().find(|e| e.name == "beta.txt").unwrap().is_dir);

    // Profundidad 4 sí lista; la 5 no (mismo tope que `build_tree`).
    let depth4 = list_tree_children(&root, &root.join("a/b/c/d")).unwrap();
    let names4: Vec<&str> = depth4.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names4, ["e", "file.txt"]);
    let depth5 = list_tree_children(&root, &root.join("a/b/c/d/e")).unwrap();
    assert!(
        depth5.is_empty(),
        "más allá de la profundidad 5 no se enumeran hijos"
    );

    // Fuera de la raíz → error de contención (§48).
    let outside = dir.path().parent().unwrap_or(Path::new("/"));
    let error = list_tree_children(&root, outside).unwrap_err();
    assert!(
        matches!(error, OpsError::Path(PathError::OutsideProject { .. })),
        "{error:?}"
    );
}

/// P2-11: los nombres que no son UTF-8 se rechazan con error claro
/// (§12.5, sin conversiones silenciosas) — igual que el listado del árbol
/// que consumía el explorador.
#[cfg(unix)]
#[test]
fn list_tree_children_rejects_non_utf8_names() {
    use std::os::unix::ffi::OsStrExt;

    let dir = TestDir::new("lazy-tree-utf8");
    let root = fs::canonicalize(dir.path()).unwrap();
    let bad = root.join(std::ffi::OsStr::from_bytes(b"bad-\xff.txt"));
    if fs::write(&bad, "x").is_err() {
        // APFS y otros filesystems rechazan nombres que no son UTF-8.
        eprintln!("skip: el filesystem rechaza nombres no UTF-8");
        return;
    }

    let error = list_tree_children(&root, &root).unwrap_err();
    assert!(matches!(error, OpsError::NotUtf8 { .. }), "{error:?}");
    assert!(error.to_string().contains("UTF-8"));
}

// ---------------------------------------------------------------- preview ---

#[test]
fn preview_classifies_text_binary_and_images() {
    let (dir, root) = project("preview");
    create_project_path(&root, "Input/core.fasta", ">seq1\nACGT\n", false).unwrap();
    assert_eq!(
        preview_file(&root, "Input/core.fasta").unwrap(),
        FilePreview::Text {
            content: String::from(">seq1\nACGT\n")
        }
    );

    // NUL → binario (pinned: test_preview_keeps_binary_files_closed).
    fs::write(root.join("Input/data.bin"), [0x00, 0x01, 0x02]).unwrap();
    assert_eq!(
        preview_file(&root, "Input/data.bin").unwrap(),
        FilePreview::Binary
    );

    // UTF-8 inválido → binario.
    fs::write(root.join("Input/raw.dat"), [0xff, 0xfe]).unwrap();
    assert_eq!(
        preview_file(&root, "Input/raw.dat").unwrap(),
        FilePreview::Binary
    );

    // CRLF → LF.
    fs::write(root.join("Input/crlf.txt"), b"a\r\nb\r\n").unwrap();
    assert_eq!(
        preview_file(&root, "Input/crlf.txt").unwrap(),
        FilePreview::Text {
            content: String::from("a\nb\n")
        }
    );

    // PNG → imagen con su tipo media (pinned: test_preview_opens_png_as_image).
    let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    fs::write(root.join("Input/plot.png"), &png).unwrap();
    assert_eq!(
        preview_file(&root, "Input/plot.png").unwrap(),
        FilePreview::Image {
            bytes: png,
            media: "image/png"
        }
    );
    fs::write(root.join("Input/photo.jpg"), b"J").unwrap();
    assert_eq!(
        preview_file(&root, "Input/photo.jpg").unwrap(),
        FilePreview::Image {
            bytes: b"J".to_vec(),
            media: "image/jpeg"
        }
    );
    fs::write(root.join("Input/diagram.svg"), b"<svg/>").unwrap();
    assert_eq!(
        preview_file(&root, "Input/diagram.svg").unwrap(),
        FilePreview::Image {
            bytes: b"<svg/>".to_vec(),
            media: "image/svg+xml"
        }
    );

    // Inexistente / fuera del proyecto / directorio → missing.
    assert_eq!(
        preview_file(&root, "Input/nope.txt").unwrap(),
        FilePreview::Missing
    );
    assert_eq!(
        preview_file(&root, "../outside.txt").unwrap(),
        FilePreview::Missing
    );
    assert_eq!(preview_file(&root, "Input").unwrap(), FilePreview::Missing);
    drop(dir);
}

// --------------------------------------------------------------- listings ---

#[test]
fn listing_skips_dot_entries_sorts_and_maps_non_dirs_to_parent() {
    let dir = TestDir::new("listing");
    let root = fs::canonicalize(dir.path()).unwrap();
    for name in ["Beta", "alpha", ".hidden"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    for name in ["z.txt", "a.txt", ".dot", "readme.md"] {
        fs::write(root.join(name), "x").unwrap();
    }

    let list = list_directories(Some(&root)).unwrap();
    assert_eq!(list.path, root);
    assert_eq!(list.parent, root.parent().map(Path::to_path_buf));
    let names: Vec<&str> = list
        .directories
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["alpha", "Beta"]);
    assert_eq!(list.directories[0].path, root.join("alpha"));

    // Un fichero como punto de partida lista su contenedora (no-dir → parent).
    let from_file = list_directories(Some(&root.join("a.txt"))).unwrap();
    assert_eq!(from_file.path, root);

    // Inexistente → NotFound.
    let err = list_directories(Some(&root.join("ghost"))).unwrap_err();
    assert!(matches!(err, OpsError::NotFound { .. }), "{err:?}");

    // Ficheros con y sin filtro de extensiones.
    fs::write(root.join("LICENSE"), "l").unwrap();
    let filtered = list_filesystem(Some(&root), Some(&[".txt"])).unwrap();
    let dir_names: Vec<&str> = filtered
        .directories
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(dir_names, ["alpha", "Beta"]);
    let file_names: Vec<&str> = filtered
        .files
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(file_names, ["a.txt", "z.txt"]);

    let all = list_filesystem(Some(&root), None).unwrap();
    let file_names: Vec<&str> = all.files.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(file_names, ["a.txt", "LICENSE", "readme.md", "z.txt"]);
    assert!(file_names.iter().all(|name| !name.starts_with('.')));

    // Sin argumento: directorio personal por defecto.
    let home = list_directories(None).unwrap();
    assert!(home.path.is_dir());
    drop(dir);
}

#[cfg(unix)]
#[test]
fn listing_resolves_symlinked_directories_to_their_target() {
    let dir = TestDir::new("listing-link");
    let root = fs::canonicalize(dir.path()).unwrap();
    let outside = TestDir::new("listing-link-outside");
    fs::create_dir(outside.path().join("real-dir")).unwrap();
    if !symlink_or_skip(&outside.path().join("real-dir"), &root.join("linked")) {
        return;
    }

    let list = list_directories(Some(&root)).unwrap();
    let linked = list
        .directories
        .iter()
        .find(|entry| entry.name == "linked")
        .expect("el enlace a carpeta se lista");
    assert_eq!(
        linked.path,
        fs::canonicalize(outside.path().join("real-dir")).unwrap()
    );
}
