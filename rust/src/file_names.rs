//! Names of the files that sit beside the exe, and the one-time adoption of
//! the pre-v3.1.0 names (the app was called DualSense Mapper until then).
//!
//! Adoption runs only when the new file is missing and the old one exists:
//! the old file is copied to `<old>.bak`, then renamed to the new name (same
//! directory, so the rename is atomic). It never overwrites a new file and
//! never fails startup: on any error the old file is left as it was and the
//! caller reads it in place.

use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "padtokbm.json";
pub const UI_PREFS_FILE: &str = "padtokbm.ui.json";
pub const LEGACY_CONFIG_FILE: &str = "dualsense-mapper.json";
pub const LEGACY_UI_PREFS_FILE: &str = "dualsense-mapper.ui.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adoption {
    /// No old file: nothing to do.
    NoLegacy,
    /// The new file already exists; neither file was touched.
    /// `legacy_present`: the old file is still there too (logged once).
    AlreadyNew { legacy_present: bool },
    /// The old file now lives under the new name; `backup` is a copy of it.
    Adopted { from: PathBuf, to: PathBuf, backup: PathBuf },
    /// Copy or rename failed; the old file is unchanged.
    Failed { legacy: PathBuf, error: String },
}

pub fn adopt_legacy(dir: &Path, legacy: &str, new: &str) -> Adoption {
    let from = dir.join(legacy);
    let to = dir.join(new);
    if to.exists() {
        return Adoption::AlreadyNew { legacy_present: from.is_file() };
    }
    if !from.is_file() {
        return Adoption::NoLegacy;
    }
    let backup = dir.join(format!("{legacy}.bak"));
    let result = std::fs::copy(&from, &backup).and_then(|_| std::fs::rename(&from, &to));
    match result {
        Ok(()) => Adoption::Adopted { from, to, backup },
        Err(e) => Adoption::Failed { legacy: from, error: e.to_string() },
    }
}

/// The config path used when no `--config` is given, after adopting an old
/// `dualsense-mapper.json` in `dir`. If adoption failed, the old file is read
/// in place so the user keeps their bindings.
pub fn default_config_path(dir: &Path) -> (PathBuf, Adoption) {
    let got = adopt_legacy(dir, LEGACY_CONFIG_FILE, CONFIG_FILE);
    let path = match &got {
        Adoption::Failed { legacy, .. } => legacy.clone(),
        _ => dir.join(CONFIG_FILE),
    };
    (path, got)
}

/// Startup adoption of the config in `dir` (the exe dir when no `--config`
/// is given), logged. Returns the config path to use (see
/// [`default_config_path`]). The old ui-prefs files are not renamed any more:
/// `gui::ui_prefs::absorb` folds them into the config (v3.4.0).
pub fn adopt_all(dir: &Path) -> PathBuf {
    let (path, got) = default_config_path(dir);
    log_adoption(&got, LEGACY_CONFIG_FILE, CONFIG_FILE);
    path
}

/// One log line for an adoption outcome of `legacy` → `new` (nothing for
/// the quiet cases).
pub fn log_adoption(got: &Adoption, legacy: &str, new: &str) {
    match got {
        Adoption::Adopted { from, to, backup } => tracing::info!(
            "renamed {} to {} (copy kept as {})",
            from.display(),
            to.display(),
            backup.display()
        ),
        Adoption::Failed { legacy, error } => {
            tracing::warn!("could not rename {}: {error}; using it in place", legacy.display())
        }
        Adoption::AlreadyNew { legacy_present: true } => {
            // Only the new file is read; say so once, so a user editing the
            // old one knows why nothing changes.
            tracing::info!("using {new}; {legacy} left untouched");
        }
        Adoption::NoLegacy | Adoption::AlreadyNew { legacy_present: false } => {}
    }
}
