// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Ámbito de ventana (ISSUE-021): standalone tipo Notepad++ o de proyecto.
//!
//! - Una ventana **standalone** no tiene carpeta asociada y acepta cualquier
//!   archivo —nuevo o existente— como pestaña (§12), como Notepad++.
//! - Una ventana de **proyecto** se abre con una carpeta y sus archivos se
//!   validan contra su raíz (§48). Abrir un archivo fuera del proyecto se
//!   resuelve con una ventana standalone nueva, nunca rechazando al usuario.

use opm_fs::resolve_in_project;
use std::path::{Path, PathBuf};

/// Ámbito de una ventana de OPM (ISSUE-021).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowScope {
    /// Sin carpeta asociada: archivos sueltos (§12).
    Standalone,
    /// Carpeta del proyecto; sus rutas se validan contra la raíz (§48).
    Project(PathBuf),
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
    /// Standalone acepta cualquier archivo (Notepad++); una ventana de
    /// proyecto solo archivos que resuelven dentro de su raíz (§48: también
    /// rechaza symlinks que apuntan fuera).
    pub fn contains(&self, path: &Path) -> bool {
        match self {
            Self::Standalone => true,
            Self::Project(root) => resolve_in_project(root, path).is_ok(),
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

    /// Raíz para la validación de rutas §48 (`None` = standalone).
    pub fn project_root(&self) -> Option<PathBuf> {
        match self {
            Self::Standalone => None,
            Self::Project(root) => Some(root.clone()),
        }
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
    fn standalone_acepta_cualquier_archivo() {
        // Notepad++: archivos nuevos o existentes, dentro o fuera de carpetas.
        let dir = TestDir::new();
        let scope = WindowScope::Standalone;
        assert!(scope.contains(&dir.0.join("cualquier.md")));
        assert!(scope.contains(&dir.0.join("no/existe/todavia.md")));
        assert_eq!(
            scope.target_for(&dir.0.join("lo_que_sea.md")),
            OpenTarget::CurrentWindow
        );
        assert_eq!(scope.project_root(), None);
    }

    #[test]
    fn proyecto_abre_lo_suyo_en_su_ventana() {
        let dir = TestDir::new();
        let root = dir.0.join("proyecto");
        fs::create_dir_all(root.join("cap")).unwrap();
        fs::write(root.join("cap/doc.md"), "x\n").unwrap();

        let scope = WindowScope::Project(root.clone());
        assert_eq!(
            scope.target_for(&root.join("cap/doc.md")),
            OpenTarget::CurrentWindow
        );
        // Un archivo nuevo dentro del proyecto también cabe en la ventana.
        assert_eq!(
            scope.target_for(&root.join("nuevo.md")),
            OpenTarget::CurrentWindow
        );
        assert_eq!(scope.project_root(), Some(root));
    }

    #[test]
    fn proyecto_manda_lo_ajeno_a_una_ventana_standalone() {
        let dir = TestDir::new();
        let root = dir.0.join("proyecto");
        let fuera = dir.0.join("suelto.md");
        fs::create_dir_all(&root).unwrap();
        fs::write(&fuera, "x\n").unwrap();

        let scope = WindowScope::Project(root);
        // Absoluto fuera, traversal y symlink que sale del proyecto (§48).
        for path in [
            fuera.clone(),
            dir.0.join("proyecto/../../suelto.md"),
            dir.0.join("proyecto/enlace.md"),
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
