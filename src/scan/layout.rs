//! Restos del sistema de backup-layout heredado de upstream Ludusavi.
//!
//! El fork no usa este formato (carpetas + `mapping.yaml` + full/diff backups).
//! En su lugar usa un único `<game>.zip` por juego en `config.backup.path`
//! gestionado por `src/sync/operations.rs`.
//!
//! Este módulo conserva solo:
//!   - `escape_folder_name`: util usado por `manifest.rs` para nombrar manifests
//!     secundarios descargados.
//!   - `BackupLayout` shell: el constructor y `restorable_game_set()` los usa
//!     `TitleFinder::new` en `sync/operations.rs` y `sync/daemon.rs`. En el fork
//!     siempre devuelve un set vacío.

use std::collections::BTreeSet;

use crate::{path::StrictPath, prelude::INVALID_FILE_CHARS};

const SAFE: &str = "_";

pub fn escape_folder_name(name: &str) -> String {
    let mut escaped = String::from(name);

    // Technically, dots should be fine as long as the folder name isn't
    // exactly `.` or `..`. However, leading dots will often cause items
    // to be hidden by default, which could be confusing for users, so we
    // escape those. And Windows Explorer has a fun bug where, if you try
    // to open a folder whose name ends with a dot, then it will say that
    // the folder no longer exists at the moment when you press enter,
    // even though the folder still does exist.
    if escaped.starts_with('.') {
        escaped.replace_range(..1, SAFE);
    }
    if escaped.ends_with('.') {
        escaped.replace_range(escaped.len() - 1.., SAFE);
    }

    escaped
        .replace(INVALID_FILE_CHARS, SAFE)
        .replace(['\n', '\r'], SAFE)
}

/// Shell residual del sistema de backup-layout. En el fork no rastrea juegos
/// (no hay `mapping.yaml` en disco) — `restorable_game_set()` devuelve siempre
/// un set vacío. Se conserva porque `TitleFinder::new` espera ese parámetro.
#[derive(Clone, Debug, Default)]
pub struct BackupLayout {
    pub base: StrictPath,
}

impl BackupLayout {
    pub fn new(base: StrictPath) -> Self {
        Self { base }
    }

    pub fn restorable_game_set(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
}
