// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Ámbito de ventana (ISSUE-021, pipeline-storage): cuatro tipos de ventana.
//!
//! - Una ventana **standalone** no tiene carpeta asociada y acepta cualquier
//!   archivo —nuevo o existente— como pestaña (§12), como Notepad++.
//! - Una ventana **design** diseña un pipeline de la librería SIN carpeta de
//!   trabajo: no tiene raíz, así que acepta cualquier archivo como pestaña.
//! - Una ventana **folder** es explorador puro sobre una carpeta (sin
//!   `run.json`): sus rutas se validan contra la raíz (§48).
//! - Una ventana **work** añade el dominio de ejecución sobre la carpeta que
//!   contiene `.pipeline-manager/run.json`; sus rutas también se validan
//!   contra la raíz (§48). Abrir un archivo fuera de la raíz se resuelve con
//!   una ventana standalone nueva, nunca rechazando al usuario.

use opm_fs::resolve_in_project;
use std::path::{Path, PathBuf};

/// Ámbito de una ventana de OPM (ISSUE-021).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowScope {
    /// Sin carpeta asociada: archivos sueltos (§12).
    Standalone,
    /// Diseño de pipeline sin carpeta de trabajo (solo librería `.pipeline`).
    Design,
    /// Carpeta explorada sin `run.json`; sus rutas se validan contra la raíz (§48).
    Folder(PathBuf),
    /// Carpeta de trabajo con `.pipeline-manager/run.json` (raíz §48).
    Work(PathBuf),
}

/// Destino de un archivo que se quiere abrir desde una ventana (ISSUE-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    /// Pestaña en la ventana actual.
    CurrentWindow,
    /// Ventana standalone nueva: el archivo está fuera del ámbito actual.
    NewStandaloneWindow,
}

impl WindowScope {
    /// ¿Puede `path` vivir en esta ventana?
    ///
    /// Standalone y Design aceptan cualquier archivo (Notepad++/diseño); una
    /// ventana con carpeta solo archivos que resuelven dentro de su raíz
    /// (§48: también rechaza symlinks que apuntan fuera).
    pub fn contains(&self, path: &Path) -> bool {
        match self {
            Self::Standalone | Self::Design => true,
            Self::Folder(root) | Self::Work(root) => resolve_in_project(root, path).is_ok(),
        }
    }

    /// Dónde se abre `path` (ISSUE-021): dentro del ámbito, en esta ventana;
    /// fuera del ámbito, en una ventana standalone nueva.
    pub fn target_for(&self, path: &Path) -> OpenTarget {
        if self.contains(path) {
            OpenTarget::CurrentWindow
        } else {
            OpenTarget::NewStandaloneWindow
        }
    }

    /// Raíz para la validación de rutas §48 y para el explorador
    /// (`None` = standalone/design).
    pub fn project_root(&self) -> Option<PathBuf> {
        match self {
            Self::Standalone | Self::Design => None,
            Self::Folder(root) | Self::Work(root) => Some(root.clone()),
        }
    }

    /// ¿Esta ventana diseña pipelines (canvas + panel editables)?
    pub fn is_design(&self) -> bool {
        matches!(self, Self::Design)
    }

    /// ¿Esta ventana ejecuta trabajo (explorador + logs + run/stop)?
    pub fn is_work(&self) -> bool {
        matches!(self, Self::Work(_))
    }

    /// ¿Tiene superficie de pipeline (página «Pipeline», lienzo, panel)?
    pub fn has_pipeline_surface(&self) -> bool {
        self.is_design() || self.is_work()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "opm-scope-{}-{}",
                std::process::id(),
                nanos
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn standalone_y_design_aceptan_cualquier_archivo() {
        // Notepad++/diseño: archivos nuevos o existentes, dentro o fuera.
        let dir = TestDir::new();
        for scope in [WindowScope::Standalone, WindowScope::Design] {
            assert!(scope.contains(&dir.0.join("cualquier.md")));
            assert!(scope.contains(&dir.0.join("no/existe/todavia.md")));
            assert_eq!(
                scope.target_for(&dir.0.join("lo_que_sea.md")),
                OpenTarget::CurrentWindow
            );
            assert_eq!(scope.project_root(), None);
        }
        // Superficies: design dibuja pipeline sin explorador; standalone nada.
        assert!(WindowScope::Design.has_pipeline_surface());
        assert_eq!(WindowScope::Design.project_root(), None);
        assert!(!WindowScope::Standalone.has_pipeline_surface());
    }

    #[test]
    fn folder_abre_lo_suyo_en_su_ventana() {
        let dir = TestDir::new();
        let root = dir.0.join("carpeta");
        fs::create_dir_all(root.join("cap")).unwrap();
        fs::write(root.join("cap/doc.md"), "x\n").unwrap();

        let scope = WindowScope::Folder(root.clone());
        assert_eq!(
            scope.target_for(&root.join("cap/doc.md")),
            OpenTarget::CurrentWindow
        );
        // Un archivo nuevo dentro de la carpeta también cabe en la ventana.
        assert_eq!(
            scope.target_for(&root.join("nuevo.md")),
            OpenTarget::CurrentWindow
        );
        assert_eq!(scope.project_root(), Some(root));
        // Solo explorador: sin superficie de pipeline.
        assert!(!scope.has_pipeline_surface());
        assert!(!scope.is_work());
    }

    #[test]
    fn work_conserva_raiz_y_superficie_de_ejecucion() {
        let dir = TestDir::new();
        let root = dir.0.join("trabajo");
        fs::create_dir_all(&root).unwrap();

        let scope = WindowScope::Work(root.clone());
        assert_eq!(scope.project_root(), Some(root));
        assert!(scope.is_work());
        assert!(scope.has_pipeline_surface());
        assert!(!scope.is_design());
    }

    #[test]
    fn carpeta_manda_lo_ajeno_a_una_ventana_standalone() {
        let dir = TestDir::new();
        let root = dir.0.join("carpeta");
        let fuera = dir.0.join("suelto.md");
        fs::create_dir_all(&root).unwrap();
        fs::write(&fuera, "x\n").unwrap();

        let scope = WindowScope::Folder(root);
        // Absoluto fuera, traversal y symlink que sale de la carpeta (§48).
        for path in [
            fuera.clone(),
            dir.0.join("carpeta/../../suelto.md"),
            dir.0.join("carpeta/enlace.md"),
        ] {
            if path.ends_with("enlace.md") {
                symlink(&fuera, &path).unwrap();
            }
            assert_eq!(
                scope.target_for(&path),
                OpenTarget::NewStandaloneWindow,
                "{path:?} debería ir a ventana standalone"
            );
        }
    }
}
