//! The board's UI state: the pad it showed last.
//!
//! Since v3.4.0 it lives in padtokbm.json (`Config::last_pad`), so the exe
//! keeps one file beside it. Up to v3.3.0 it sat in a second file there,
//! `padtokbm.ui.json` (before v3.1.0 `dualsense-mapper.ui.json`, whose rename
//! left a `.bak`). [`absorb`] runs once at startup: an old file hands its last
//! pad to the config and turns its `log_events` off (v3.4.1), then all of them
//! are deleted. Never blocks startup.

use crate::config_io::{write_atomic, ConfigDoc};
use crate::file_names::{LEGACY_UI_PREFS_FILE, UI_PREFS_FILE};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The IPC shape of `get_ui_prefs` / `set_ui_prefs` (unchanged from the
/// ui-prefs file, whose other keys are ignored).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default)]
    pub last_pad: Option<String>,
}

/// The old ui-prefs files, newest first; the first readable one is used.
const OLD_FILES: [&str; 2] = [UI_PREFS_FILE, LEGACY_UI_PREFS_FILE];
/// Left by the v3.1.0 rename of the pre-rename file; deleted, never read.
const OLD_BACKUP: &str = "dualsense-mapper.ui.json.bak";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Absorb {
    /// No old file beside the config.
    Nothing,
    /// The old files are gone; `last_pad` is the pad written into the
    /// config (`None`: nothing to hand over, or the config already had one);
    /// `log_off`: the config's `log_events` was on and is now off.
    Done { last_pad: Option<String>, log_off: bool },
    /// The config could not take the pad (unreadable / invalid / not
    /// writable): the old files stay so the next start can retry.
    Kept { error: String },
}

fn old_pad(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<UiPrefs>(&text).ok()?.last_pad
}

/// Move the last pad from an old ui-prefs file beside `config_path` into the
/// config (unless it already has one), turn the config's `log_events` off,
/// then delete the old files. Logged.
pub fn absorb(config_path: &Path) -> Absorb {
    let dir = config_path.parent().unwrap_or_else(|| Path::new(""));
    let present: Vec<_> = OLD_FILES
        .iter()
        .chain(std::iter::once(&OLD_BACKUP))
        .map(|f| dir.join(f))
        .filter(|p| p.is_file())
        .collect();
    if present.is_empty() {
        return Absorb::Nothing;
    }
    // An old file means the config was written before v3.4.0, when the first
    // start turned logging on; this start turns it off (v3.4.1). Once: the
    // old files go below, so a user who turns it back on keeps it.
    let pad = OLD_FILES.iter().find_map(|f| old_pad(&dir.join(f)));
    let result = ConfigDoc::load(config_path).and_then(|mut doc| {
        let moved = match pad {
            Some(pad) if doc.typed().last_pad.is_none() => {
                doc.set_last_pad(&pad);
                Some(pad)
            }
            _ => None,
        };
        let log_off = doc.typed().log_events;
        if log_off {
            doc.set_log_events(false);
        }
        if moved.is_some() || log_off {
            write_atomic(config_path, &doc)?;
        }
        Ok((moved, log_off))
    });
    let (moved, log_off) = match result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("kept {}: could not update {}: {e:#}", UI_PREFS_FILE, config_path.display());
            return Absorb::Kept { error: format!("{e:#}") };
        }
    };
    if log_off {
        tracing::info!("turned log_events off in {} (on by default before v3.4.0)", config_path.display());
    }
    for p in &present {
        match std::fs::remove_file(p) {
            Ok(()) => tracing::info!("removed {} (its last pad now lives in {})", p.display(), config_path.display()),
            Err(e) => tracing::warn!("could not remove {}: {e}", p.display()),
        }
    }
    Absorb::Done { last_pad: moved, log_off }
}
