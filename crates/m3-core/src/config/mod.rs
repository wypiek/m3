//! Configuration files under `~/.config/m3/`.
//!
//! Every file is a [`ConfigFile`]: it has defaults, a schema version, semantic
//! validation and (optionally) migrations from older schema versions.
//!
//! Guarantees:
//! * Loading never writes to disk. Migrations happen in memory.
//! * An invalid file is never silently replaced. Callers get a
//!   [`ConfigError`] with a human-readable explanation and keep using their
//!   previous (or default) configuration.
//! * [`save`] writes atomically (temp file + rename), refuses to write values
//!   that fail validation, refuses to downgrade files written by a newer m3,
//!   backs up files before upgrading their schema, and merges into the existing
//!   document so the user's comments and formatting survive.

mod compositor;
mod keybindings;
mod lock;
mod notifications;
mod panels;
mod theme;
mod widgets;
mod workspaces;

pub use compositor::*;
pub use keybindings::*;
pub use lock::*;
pub use notifications::*;
pub use panels::*;
pub use theme::*;
pub use widgets::*;
pub use workspaces::*;

use serde::{Serialize, de::DeserializeOwned};
use std::io::Write;
use std::path::{Path, PathBuf};

pub trait ConfigFile: Serialize + DeserializeOwned + Default + Clone {
    /// File name inside the config directory, e.g. `config.toml`.
    const FILE_NAME: &'static str;
    /// Current schema version written by this build.
    const SCHEMA_VERSION: u32;

    /// Semantic validation. Returns one human-readable message per problem.
    fn validate(&self) -> Vec<String> {
        Vec::new()
    }

    /// Upgrade a raw document from schema version `from` to `from + 1`.
    fn migrate_step(_doc: &mut toml::Table, from: u32) -> Result<(), String> {
        Err(format!("no migration from schema version {from}"))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no configuration directory: $HOME is not set")]
    NoConfigDir,
    #[error("cannot access {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path} is not valid TOML: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{path} has invalid settings:\n  - {}", problems.join("\n  - "))]
    Invalid { path: PathBuf, problems: Vec<String> },
    #[error(
        "{path} uses schema version {found}, but this m3 only understands up to {supported}; \
         it was probably written by a newer m3 and will not be modified"
    )]
    TooNew { path: PathBuf, found: u32, supported: u32 },
    #[error("cannot migrate {path}: {message}")]
    Migration { path: PathBuf, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The file does not exist; built-in defaults are in use.
    Defaults,
    /// Loaded from disk.
    File,
}

#[derive(Debug, Clone)]
pub struct Loaded<T> {
    pub value: T,
    pub source: Source,
    /// Non-fatal diagnostics: unknown keys, in-memory migrations.
    pub warnings: Vec<String>,
}

pub fn path_of<T: ConfigFile>(dir: &Path) -> PathBuf {
    dir.join(T::FILE_NAME)
}

/// The default config directory, see [`crate::paths::config_dir`].
pub fn default_dir() -> Result<PathBuf, ConfigError> {
    crate::paths::config_dir().ok_or(ConfigError::NoConfigDir)
}

fn schema_version_of(table: &toml::Table) -> Option<u32> {
    table.get("schema_version").and_then(|v| v.as_integer()).and_then(|v| u32::try_from(v).ok())
}

/// Parse and validate a configuration document.
pub fn parse<T: ConfigFile>(path: &Path, text: &str) -> Result<Loaded<T>, ConfigError> {
    let mut table: toml::Table =
        toml::from_str(text).map_err(|e| ConfigError::Parse { path: path.to_owned(), message: e.to_string() })?;

    let mut warnings = Vec::new();
    let found = schema_version_of(&table).unwrap_or(T::SCHEMA_VERSION);
    if found > T::SCHEMA_VERSION {
        return Err(ConfigError::TooNew { path: path.to_owned(), found, supported: T::SCHEMA_VERSION });
    }
    if found < T::SCHEMA_VERSION {
        for v in found..T::SCHEMA_VERSION {
            T::migrate_step(&mut table, v)
                .map_err(|message| ConfigError::Migration { path: path.to_owned(), message })?;
        }
        warnings.push(format!(
            "{} uses schema version {found}; it was upgraded in memory to version {} and will be \
             rewritten (with a backup) the next time settings are saved",
            path.display(),
            T::SCHEMA_VERSION
        ));
    }

    // The version is file metadata, not part of the typed configuration.
    table.remove("schema_version");
    let mut unknown = Vec::new();
    let value: T = serde_ignored::deserialize(toml::Value::Table(table), |p| unknown.push(p.to_string()))
        .map_err(|e: toml::de::Error| ConfigError::Parse { path: path.to_owned(), message: e.to_string() })?;
    for key in unknown {
        warnings.push(format!("{}: unknown setting `{key}` is ignored", path.display()));
    }

    let problems = value.validate();
    if !problems.is_empty() {
        return Err(ConfigError::Invalid { path: path.to_owned(), problems });
    }
    Ok(Loaded { value, source: Source::File, warnings })
}

/// Load `T` from `dir`. A missing file yields defaults; anything else that
/// goes wrong is an error and the caller decides what to fall back to.
pub fn load<T: ConfigFile>(dir: &Path) -> Result<Loaded<T>, ConfigError> {
    let path = path_of::<T>(dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&path, &text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Loaded { value: T::default(), source: Source::Defaults, warnings: Vec::new() })
        }
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

/// Load `T`, logging problems and falling back to `fallback` (typically the
/// previously active configuration, or defaults at startup).
pub fn load_or<T: ConfigFile>(dir: &Path, fallback: T) -> (T, Option<ConfigError>) {
    match load::<T>(dir) {
        Ok(l) => {
            for w in &l.warnings {
                tracing::warn!("{w}");
            }
            (l.value, None)
        }
        Err(e) => {
            tracing::error!("{e}; keeping the previous configuration");
            (fallback, Some(e))
        }
    }
}

/// Save `value` to `dir`, preserving comments and formatting of the existing
/// file where possible.
pub fn save<T: ConfigFile>(dir: &Path, value: &T) -> Result<(), ConfigError> {
    let path = path_of::<T>(dir);
    let problems = value.validate();
    if !problems.is_empty() {
        return Err(ConfigError::Invalid { path, problems });
    }

    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| ConfigError::Io { path, source }
    };
    std::fs::create_dir_all(dir).map_err(io(dir))?;

    let mut new_doc: toml_edit::DocumentMut = toml_edit::ser::to_document(value)
        .map_err(|e| ConfigError::Parse { path: path.clone(), message: e.to_string() })?;
    new_doc.insert("schema_version", toml_edit::value(i64::from(T::SCHEMA_VERSION)));

    let output = match std::fs::read_to_string(&path) {
        Ok(existing) => match existing.parse::<toml_edit::DocumentMut>() {
            Ok(mut doc) => {
                let old_version = doc
                    .get("schema_version")
                    .and_then(|v| v.as_integer())
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(T::SCHEMA_VERSION);
                if old_version > T::SCHEMA_VERSION {
                    return Err(ConfigError::TooNew { path, found: old_version, supported: T::SCHEMA_VERSION });
                }
                if old_version < T::SCHEMA_VERSION {
                    let backup = path.with_extension(format!("toml.v{old_version}.bak"));
                    std::fs::copy(&path, &backup).map_err(io(&backup))?;
                    tracing::info!("backed up {} to {}", path.display(), backup.display());
                }
                merge_tables(doc.as_table_mut(), new_doc.as_table());
                // Keep schema_version first for readability on fresh keys.
                doc.to_string()
            }
            Err(e) => {
                let backup = path.with_extension("toml.invalid.bak");
                std::fs::copy(&path, &backup).map_err(io(&backup))?;
                tracing::warn!(
                    "{} was not valid TOML ({e}); saved a copy to {} before rewriting it",
                    path.display(),
                    backup.display()
                );
                new_doc.to_string()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut doc = new_doc;
            reorder_version_first(&mut doc);
            doc.to_string()
        }
        Err(source) => return Err(ConfigError::Io { path, source }),
    };

    atomic_write(&path, output.as_bytes()).map_err(io(&path))
}

fn reorder_version_first(doc: &mut toml_edit::DocumentMut) {
    if let Some(v) = doc.remove("schema_version") {
        let rest: Vec<(String, toml_edit::Item)> = doc.iter().map(|(k, v)| (k.to_owned(), v.clone())).collect();
        doc.clear();
        doc.insert("schema_version", v);
        for (k, v) in rest {
            doc.insert(&k, v);
        }
    }
}

/// Merge `src` into `dst`: values are replaced (keeping the comments attached
/// to them), tables are merged recursively, keys missing from `src` are removed.
fn merge_tables(dst: &mut dyn toml_edit::TableLike, src: &dyn toml_edit::TableLike) {
    let stale: Vec<String> = dst.iter().map(|(k, _)| k.to_owned()).filter(|k| src.get(k).is_none()).collect();
    for k in stale {
        dst.remove(&k);
    }
    for (key, src_item) in src.iter() {
        match dst.get_mut(key) {
            Some(dst_item) => merge_item(dst_item, src_item),
            None => {
                dst.insert(key, src_item.clone());
            }
        }
    }
}

fn merge_item(dst: &mut toml_edit::Item, src: &toml_edit::Item) {
    if let (Some(d), Some(s)) = (dst.as_table_like_mut(), src.as_table_like()) {
        merge_tables(d, s);
        return;
    }
    match (dst.as_value_mut(), src.as_value()) {
        (Some(d), Some(s)) => {
            if d.to_string().trim() != s.to_string().trim() {
                let decor = d.decor().clone();
                *d = s.clone();
                *d.decor_mut() = decor;
            }
        }
        _ => *dst = src.clone(),
    }
}

/// Write `bytes` to `path` atomically: write a sibling temp file, fsync it and
/// rename it over the target. The temp file is created exclusively with mode
/// 0600 and the final file keeps that restrictive mode.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Helper for validators.
pub(crate) fn check_range<T: PartialOrd + std::fmt::Display>(
    problems: &mut Vec<String>,
    name: &str,
    value: T,
    min: T,
    max: T,
) {
    if !(value >= min && value <= max) {
        problems.push(format!("`{name}` must be between {min} and {max} (found {value})"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        assert!(CompositorConfig::default().validate().is_empty());
        assert!(Keybindings::default().validate().is_empty());
        assert!(WorkspacesConfig::default().validate().is_empty());
        assert!(ThemeConfig::default().validate().is_empty());
        assert!(PanelsConfig::default().validate().is_empty());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let l = load::<CompositorConfig>(dir.path()).unwrap();
        assert_eq!(l.source, Source::Defaults);
    }

    #[test]
    fn partial_file_fills_defaults_and_reports_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[input.keyboard]\nrepeat_rate = 40\nbogus = 1\n").unwrap();
        let l = load::<CompositorConfig>(dir.path()).unwrap();
        assert_eq!(l.value.input.keyboard.repeat_rate, 40);
        assert_eq!(l.value.input.keyboard.repeat_delay, InputKeyboard::default().repeat_delay);
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        assert!(l.warnings[0].contains("input.keyboard.bogus"));
    }

    #[test]
    fn invalid_values_are_rejected_with_messages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[input.keyboard]\nrepeat_rate = 9000\n").unwrap();
        match load::<CompositorConfig>(dir.path()) {
            Err(ConfigError::Invalid { problems, .. }) => {
                assert!(problems[0].contains("repeat_rate"), "{problems:?}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn syntax_errors_are_reported_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[input\n").unwrap();
        let (cfg, err) = load_or(dir.path(), CompositorConfig::default());
        assert!(matches!(err, Some(ConfigError::Parse { .. })));
        assert_eq!(cfg.input.keyboard.repeat_rate, InputKeyboard::default().repeat_rate);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "[input\n");
    }

    #[test]
    fn newer_schema_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "schema_version = 99\n").unwrap();
        assert!(matches!(load::<CompositorConfig>(dir.path()), Err(ConfigError::TooNew { .. })));
        assert!(matches!(save(dir.path(), &CompositorConfig::default()), Err(ConfigError::TooNew { .. })));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "schema_version = 99\n");
    }

    #[test]
    fn save_roundtrips_and_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "# my precious comment\n[input.keyboard]\n# rate comment\nrepeat_rate = 40\n").unwrap();
        let mut cfg = load::<CompositorConfig>(dir.path()).unwrap().value;
        cfg.input.keyboard.repeat_rate = 50;
        save(dir.path(), &cfg).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# my precious comment"), "{text}");
        assert!(text.contains("# rate comment"), "{text}");
        assert!(text.contains("repeat_rate = 50"), "{text}");
        let back = load::<CompositorConfig>(dir.path()).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.value.input.keyboard.repeat_rate, 50);
    }

    #[test]
    fn save_refuses_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = CompositorConfig::default();
        cfg.input.keyboard.repeat_rate = 0;
        assert!(matches!(save(dir.path(), &cfg), Err(ConfigError::Invalid { .. })));
        assert!(!dir.path().join("config.toml").exists());
    }

    #[test]
    fn every_file_roundtrips() {
        fn rt<T: ConfigFile + PartialEq + std::fmt::Debug>() {
            let dir = tempfile::tempdir().unwrap();
            save(dir.path(), &T::default()).unwrap();
            let l = load::<T>(dir.path()).unwrap();
            assert!(l.warnings.is_empty(), "{}: {:?}", T::FILE_NAME, l.warnings);
            assert_eq!(l.value, T::default(), "{}", T::FILE_NAME);
        }
        rt::<CompositorConfig>();
        rt::<Keybindings>();
        rt::<WorkspacesConfig>();
        rt::<ThemeConfig>();
        rt::<PanelsConfig>();
        rt::<NotificationsConfig>();
        rt::<LockConfig>();
    }

    #[test]
    fn atomic_write_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.toml");
        atomic_write(&p, b"a = 1\n").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "temp file left behind");
    }
}

#[cfg(test)]
mod doc_examples {
    use super::*;

    /// Every TOML example in docs/CONFIGURATION.md must parse, validate and
    /// contain no unknown keys — the documentation cannot drift from code.
    #[test]
    fn configuration_md_examples_are_valid() {
        let doc = include_str!("../../../../docs/CONFIGURATION.md");
        let mut checked = 0;
        for section in doc.split("\n## ").skip(1) {
            let file = section.lines().next().unwrap_or_default().trim().to_owned();
            let Some(start) = section.find("```toml\n") else { continue };
            let rest = &section[start + 8..];
            let Some(end) = rest.find("```") else { continue };
            let toml = &rest[..end];
            let path = std::path::Path::new(&file);
            let check = |r: Result<Vec<String>, ConfigError>| match r {
                Ok(w) => assert!(w.is_empty(), "{file}: {w:?}"),
                Err(e) => panic!("{file}: {e}"),
            };
            match file.as_str() {
                "config.toml" => check(parse::<CompositorConfig>(path, toml).map(|l| l.warnings)),
                "keybindings.toml" => check(parse::<Keybindings>(path, toml).map(|l| l.warnings)),
                "workspaces.toml" => check(parse::<WorkspacesConfig>(path, toml).map(|l| l.warnings)),
                "theme.toml" => check(parse::<ThemeConfig>(path, toml).map(|l| l.warnings)),
                "panels.toml" => check(parse::<PanelsConfig>(path, toml).map(|l| l.warnings)),
                "notifications.toml" => check(parse::<NotificationsConfig>(path, toml).map(|l| l.warnings)),
                "lock.toml" => check(parse::<LockConfig>(path, toml).map(|l| l.warnings)),
                "widgets.toml" => check(parse::<WidgetsConfig>(path, toml).map(|l| l.warnings)),
                _ => continue,
            }
            checked += 1;
        }
        assert_eq!(checked, 8, "expected one example per configuration file");
    }
}
