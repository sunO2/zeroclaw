//! Forward-only schema migration: V1 → V2 → V3.
//!
//! User TOML on disk is the source of truth. Each historical version (V1, V2)
//! is a partial typed lens (`schema/v{1,2}.rs`) — explicit Rust fields only for
//! what transforms between adjacent versions; everything else rides through
//! `passthrough: toml::Table`. V3 is the live `Config` in `schema.rs`.
//!
//! Public API (preserved from the previous implementation so existing callers
//! in `schema.rs`, `src/main.rs`, gateway, tools, and tests keep compiling
//! without changes):
//! - `CURRENT_SCHEMA_VERSION` — current schema version constant
//! - `V1_LEGACY_KEYS` — top-level keys whose presence implies V1 input
//! - `migrate_to_current(&str) -> Result<Config>` — high-level: TOML → V3 Config
//! - `migrate_file(&str) -> Result<Option<String>>` — pure transform; returns
//!   `Some(migrated)` if migration ran, `None` if input was already current
//! - `sync_table(toml_edit::Table, &toml::Table)` — comment-preserving
//!   reconciliation helper used by `Config::save()`

use anyhow::{Context, Result};
use std::path::Path;

use crate::schema::Config;
use crate::schema::v1::V1Config;
use crate::schema::v2::V2Config;

/// The schema version this binary writes and expects on disk.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

/// Top-level TOML keys that legacy schema versions had but V3 either
/// removed or restructured into a different shape. Used by
/// `Config::unknown_keys` to suppress false "unknown key" warnings on
/// V1/V2 configs migrating through `migrate_to_current` — every key here
/// is consumed by the V1→V2 or V2→V3 migration step, so its presence in
/// a stale-but-being-migrated config is expected, not a typo.
///
/// Sources:
/// - V1→V2 removed/renamed (top-level): `git show 1ec9c14ca:crates/zeroclaw-config/src/schema.rs`.
/// - V2→V3 removed/restructured (top-level): `git show 68a875b5b:crates/zeroclaw-config/src/schema.rs`.
pub const V1_LEGACY_KEYS: &[&str] = &[
    // V1 → V2 removed/renamed
    "api_key",
    "api_url",
    "api_path",
    "default_model_provider",
    "default_model",
    "model_providers",
    "default_temperature",
    "provider_timeout_secs",
    "provider_max_tokens",
    "extra_headers",
    "model_routes",
    "embedding_routes",
    "channels_config",
    // V2 → V3 removed or shape-changed at top level. V3's `Config::default()`
    // serialization either omits these (HashMap-typed, `skip_serializing_if`
    // empty) or doesn't carry the field at all, so without this entry the
    // unknown-key probe would flag a legitimately-migrating V2 input.
    "autonomy",
    "agent",
    "swarms",
    "cron",
];

/// Detect a config's schema version from its parsed TOML representation.
///
/// - Missing top-level `schema_version` key → V1 (pre-versioned).
/// - Integer ≥ 1 → that integer.
/// - Anything else → error.
pub fn detect_version(value: &toml::Value) -> Result<u32> {
    let table = value
        .as_table()
        .context("config root must be a TOML table")?;
    match table.get("schema_version") {
        None => Ok(1),
        Some(toml::Value::Integer(n)) if *n >= 1 => Ok(*n as u32),
        Some(other) => Err(anyhow::anyhow!(
            "schema_version must be a positive integer, got {other}"
        )),
    }
}

/// Pure migration from any supported version's TOML string into the current
/// schema version's TOML string. Returns `Ok(None)` when the input is already
/// at `CURRENT_SCHEMA_VERSION`.
///
/// Comments and decoration on keys whose dotted path survives the migration
/// are preserved via `toml_edit::DocumentMut` reconciliation (`sync_table`).
/// Keys that are renamed, removed, or restructured lose their comments — the
/// `.backup` file written by `migrate_file_in_place` retains the original
/// for manual recovery.
pub fn migrate_file(input: &str) -> Result<Option<String>> {
    let value: toml::Value = toml::from_str(input).context("failed to parse config TOML")?;
    let from = detect_version(&value)?;
    if from == CURRENT_SCHEMA_VERSION {
        return Ok(None);
    }
    if from > CURRENT_SCHEMA_VERSION {
        return Err(anyhow::anyhow!(
            "config schema_version {from} is newer than this binary supports ({CURRENT_SCHEMA_VERSION})"
        ));
    }
    let migrated_value = run_chain(value, from)?;
    let migrated_table = match migrated_value {
        toml::Value::Table(t) => t,
        _ => {
            anyhow::bail!("migrated config is not a TOML table");
        }
    };

    // Try to preserve comments by reconciling into the original DocumentMut.
    // If the original doesn't parse as toml_edit (rare — toml::from_str
    // already succeeded on it), fall back to a fresh serialization.
    if let Ok(mut doc) = input.parse::<toml_edit::DocumentMut>() {
        sync_table(doc.as_table_mut(), &migrated_table);
        Ok(Some(doc.to_string()))
    } else {
        let serialized = toml::to_string_pretty(&toml::Value::Table(migrated_table))
            .context("failed to serialize migrated config")?;
        Ok(Some(serialized))
    }
}

/// The canonical V1 fixture, embedded into the binary. Single source of
/// truth for the migration test suite (`tests/migration.rs`) and for
/// [`generate`] / the `zeroclaw config generate` CLI command. Hand-authored
/// to exercise every V1→V2 transformation rule.
const V1_FIXTURE: &str = include_str!("../fixtures/v1.toml");

/// Options for [`generate`].
#[derive(Debug, Default, Clone)]
pub struct GenerateOptions<'a> {
    /// Encrypt secret-bearing string values in the output. Works at every
    /// schema version via [`encrypt_secret_strings`], which walks the TOML
    /// and ChaCha20-Poly1305-encrypts any leaf whose key name appears in
    /// [`SECRET_KEY_NAMES`].
    pub encrypt_secrets: bool,
    /// Directory containing (or to receive) the `.secret_key` used for
    /// `enc2:` encryption. Required when `encrypt_secrets` is true. The
    /// key is created with 0o600 permissions if absent — matches how the
    /// daemon's `SecretStore` behaves on first use.
    pub secret_store_dir: Option<&'a Path>,
}

/// Generate a canonical TOML config at `target_version`, derived by
/// running the V1 fixture forward through the typed migration chain.
///
/// `target_version` must be in `1..=CURRENT_SCHEMA_VERSION`. The chain is
/// the same one used to migrate real on-disk configs — V1 fixture →
/// `V1Config::migrate` → V2 typed value → `V2Config::migrate` → V3 typed
/// value — so `generate <n>` shows exactly the shape an operator running
/// `zeroclaw config migrate` would land on if they started from the V1
/// fixture.
///
/// When [`GenerateOptions::encrypt_secrets`] is set, secret-bearing
/// string values (api_key, bot_token, access_token, etc. — see
/// [`SECRET_KEY_NAMES`]) are ChaCha20-Poly1305-encrypted with the
/// `.secret_key` under `secret_store_dir`. Works at every version.
pub fn generate(target_version: u32, opts: &GenerateOptions<'_>) -> Result<String> {
    if target_version == 0 || target_version > CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported schema version {target_version} \
             (valid: 1..={CURRENT_SCHEMA_VERSION})"
        );
    }

    let value = if target_version == 1 {
        toml::from_str::<toml::Value>(V1_FIXTURE).context("embedded V1 fixture is malformed")?
    } else {
        let v1_value: toml::Value =
            toml::from_str(V1_FIXTURE).context("embedded V1 fixture is malformed")?;
        run_chain_until(v1_value, 1, target_version)?
    };

    let mut value = value;
    if opts.encrypt_secrets {
        let store_dir = opts.secret_store_dir.context(
            "--encrypt requires a secret-store directory \
             (typically the resolved ZEROCLAW_CONFIG_DIR)",
        )?;
        let store = crate::secrets::SecretStore::new(store_dir, true);
        encrypt_secret_strings(&mut value, &store)
            .context("failed to encrypt secret-bearing fields in generated config")?;
    }

    toml::to_string_pretty(&value).context("failed to serialize generated config")
}

/// TOML keys whose string leaves are treated as secrets by
/// [`encrypt_secret_strings`]. The list is the union of every V1, V2,
/// and V3 `#[secret]`-annotated field name; matching is exact on the
/// terminal key (the last path segment) so nested `linkedin.image.*.api_key_env`
/// references are caught alongside top-level `[model_providers.<X>].api_key`.
///
/// Also catches list-of-secret cases (`paired_tokens`) and tuple-style
/// strings under known secret keys (`private_key`, `webhook_secret`).
const SECRET_KEY_NAMES: &[&str] = &[
    "api_key",
    "api_token",
    "access_token",
    "app_password",
    "app_secret",
    "app_token",
    "auth_token",
    "bearer_token",
    "bot_token",
    "channel_access_token",
    "channel_secret",
    "client_secret",
    "encrypt_key",
    "encoding_aes_key",
    "nickserv_password",
    "oauth_token",
    "password",
    "paired_tokens",
    "private_key",
    "refresh_token",
    "sasl_password",
    "secret",
    "server_password",
    "shared_secret",
    "signing_secret",
    "token",
    "verification_token",
    "verify_token",
    "webhook_secret",
];

/// Walk a TOML tree and encrypt every string leaf whose terminal key
/// name appears in [`SECRET_KEY_NAMES`]. Strings already in `enc2:` /
/// `enc:` form are left alone (idempotent). Arrays of strings under a
/// matching key (e.g. `paired_tokens`) are encrypted element-wise.
///
/// Works at every schema version because it operates on raw TOML
/// rather than the typed `#[secret]` index, which only exists for V3.
pub fn encrypt_secret_strings(
    value: &mut toml::Value,
    store: &crate::secrets::SecretStore,
) -> Result<()> {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table.iter_mut() {
                if SECRET_KEY_NAMES.contains(&key.as_str()) {
                    encrypt_in_place(child, store)
                        .with_context(|| format!("encrypting secret at key `{key}`"))?;
                } else {
                    encrypt_secret_strings(child, store)?;
                }
            }
        }
        toml::Value::Array(items) => {
            for item in items.iter_mut() {
                encrypt_secret_strings(item, store)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Encrypt the value at this slot — a string, an array of strings, or
/// a table containing strings — using the given store. Non-string leaves
/// (numbers, bools) are left alone; the operator presumably annotated a
/// non-secret field with a secret-shaped name and we don't second-guess.
fn encrypt_in_place(value: &mut toml::Value, store: &crate::secrets::SecretStore) -> Result<()> {
    match value {
        toml::Value::String(s) => {
            if !crate::secrets::SecretStore::is_encrypted(s) && !s.is_empty() {
                let encrypted = store.encrypt(s).context("encrypt string")?;
                *s = encrypted;
            }
        }
        toml::Value::Array(items) => {
            for item in items.iter_mut() {
                encrypt_in_place(item, store)?;
            }
        }
        toml::Value::Table(table) => {
            for (_, child) in table.iter_mut() {
                encrypt_secret_strings(child, store)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// High-level: arbitrary versioned TOML → fully validated V3 `Config`.
/// Runs migration if needed, then deserializes into the current `Config` type.
pub fn migrate_to_current(input: &str) -> Result<Config> {
    let value: toml::Value = toml::from_str(input).context("failed to parse config TOML")?;
    let from = detect_version(&value)?;
    let final_value = if from == CURRENT_SCHEMA_VERSION {
        value
    } else if from > CURRENT_SCHEMA_VERSION {
        return Err(anyhow::anyhow!(
            "config schema_version {from} is newer than this binary supports ({CURRENT_SCHEMA_VERSION})"
        ));
    } else {
        run_chain(value, from)?
    };
    final_value
        .try_into()
        .context("migrated config failed to deserialize as current schema")
}

/// File-API wrapper: read disk config, migrate, write `<file>.backup`
/// adjacent to the original, then atomically replace the original. Returns
/// `Ok(None)` when already current.
///
/// Backup file is `<config_filename>.backup` (joined cross-platform via
/// `Path` ops). The write path mirrors `Config::save()` so the documented
/// durability guarantee holds end-to-end:
///
/// 1. Write the migrated content to `<path>.tmp-<uuid>` and fsync it.
/// 2. Copy the original to `<path>.backup` (existing behavior; recovery
///    rope if anything later goes wrong).
/// 3. `rename(<path>.tmp, <path>)` — atomic on Unix and on modern Windows.
/// 4. Fsync the parent directory so the rename is durable.
///
/// On rename failure the temp file is removed and the backup is restored
/// over the original so the operator never observes a partial write.
pub fn migrate_file_in_place(path: &Path) -> Result<Option<MigrateReport>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    let migrated = match migrate_file(&raw)? {
        Some(s) => s,
        None => return Ok(None),
    };
    let parent = path
        .parent()
        .with_context(|| format!("config path {} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .with_context(|| format!("config path {} has no file name", path.display()))?;
    let backup_path = parent.join(format!("{file_name}.backup"));
    let temp_path = parent.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));

    // 1. Write migrated content to temp + fsync.
    {
        let mut temp = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .with_context(|| {
                format!(
                    "failed to create temporary migrated config at {}",
                    temp_path.display()
                )
            })?;
        std::io::Write::write_all(&mut temp, migrated.as_bytes()).with_context(|| {
            format!("failed to write migrated config to {}", temp_path.display())
        })?;
        temp.sync_all().with_context(|| {
            format!(
                "failed to fsync temporary migrated config at {}",
                temp_path.display()
            )
        })?;
    }

    // 2. Backup original BEFORE touching the destination. Copy gets a fresh inode.
    std::fs::copy(path, &backup_path).with_context(|| {
        format!(
            "failed to write backup {} before migration (temp file intact at {})",
            backup_path.display(),
            temp_path.display(),
        )
    })?;

    // 3. Atomic rename. On failure, restore from backup so the operator
    //    never observes a partial write.
    if let Err(rename_err) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        if backup_path.exists() {
            let _ = std::fs::copy(&backup_path, path);
        }
        return Err(anyhow::anyhow!(
            "failed to atomically replace {} with migrated config: {rename_err} \
             (backup retained at {})",
            path.display(),
            backup_path.display(),
        ));
    }

    // 4. Fsync the parent directory so the rename is durable across crashes.
    sync_directory(parent).with_context(|| {
        format!(
            "failed to fsync parent directory after migration: {}",
            parent.display()
        )
    })?;

    Ok(Some(MigrateReport {
        backup_path,
        to_version: CURRENT_SCHEMA_VERSION,
    }))
}

/// Fsync the directory entry so a subsequent rename inside it is durable.
/// No-op on platforms where directory fsync isn't a meaningful primitive.
#[allow(clippy::unused_async)] // kept sync to mirror Config::save()'s helper
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let dir = std::fs::File::open(path)
            .with_context(|| format!("failed to open directory for fsync: {}", path.display()))?;
        dir.sync_all()
            .with_context(|| format!("failed to fsync directory: {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        // Best-effort: open + drop. Windows doesn't provide a portable
        // directory-fsync primitive in std; the rename itself is durable
        // on NTFS.
        let _ = std::fs::File::open(path);
    }
    Ok(())
}

/// Result of an on-disk migration. Returned by `migrate_file_in_place` when
/// migration ran (vs. `Ok(None)` when input was already current).
#[derive(Debug, Clone)]
pub struct MigrateReport {
    pub backup_path: std::path::PathBuf,
    pub to_version: u32,
}

/// Move a legacy `<install>/workspace/` into
/// `<install>/agents/default/workspace/` (one-time migration from the
/// pre-multi-agent layout).
///
/// Idempotent: on a fresh install (no legacy dir) this is a no-op;
/// on an already-migrated install (legacy dir gone, new dir
/// populated) this is also a no-op. Mid-migration crash recovery is
/// the operator's responsibility — the function refuses to overwrite
/// a populated target dir, so a half-finished move surfaces as a
/// loud error rather than data loss.
///
/// Before any move, copies the legacy workspace contents to
/// `<install>/backup-<timestamp>/legacy-workspace/` so a rollback is
/// just `mv` back. The backup uses copy-not-rename so a partial
/// failure mid-copy does not orphan the legacy data.
///
/// Returns `Ok(true)` when a migration actually ran, `Ok(false)`
/// when nothing needed to happen.
pub fn migrate_legacy_workspace_to_default_agent(install_root: &Path) -> Result<bool> {
    let legacy = install_root.join("workspace");
    let agents_dir = install_root.join("agents");
    let new_default = agents_dir.join("default").join("workspace");

    // Fast path: legacy doesn't exist → nothing to do (fresh install or
    // already migrated).
    if !legacy.is_dir() {
        return Ok(false);
    }

    // The new path already exists AND is populated → assume migration
    // already ran and the operator (or a previous boot) hasn't
    // cleaned up the legacy dir yet. Don't touch.
    if new_default.is_dir() {
        let populated = std::fs::read_dir(&new_default)
            .map(|mut iter| iter.next().is_some())
            .unwrap_or(false);
        if populated {
            tracing::info!(
                target = %new_default.display(),
                legacy = %legacy.display(),
                "filesystem migration: target already populated; skipping move. \
                 Legacy dir can be removed manually after verifying the migration."
            );
            return Ok(false);
        }
    }

    // Pre-migration backup. Copy-not-rename so a partial failure mid-
    // copy does not orphan the legacy data. The timestamp keeps
    // backups distinct across multiple boot attempts on a broken
    // install.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let backup_dir = install_root
        .join(format!("backup-{timestamp}"))
        .join("legacy-workspace");
    std::fs::create_dir_all(&backup_dir).with_context(|| {
        format!(
            "[system] failed to create migration backup dir at {}",
            backup_dir.display()
        )
    })?;
    copy_dir_recursive(&legacy, &backup_dir).with_context(|| {
        format!(
            "[system] failed to back up legacy workspace from {} to {}",
            legacy.display(),
            backup_dir.display()
        )
    })?;
    tracing::info!(
        target = %backup_dir.display(),
        "[system] filesystem migration: legacy workspace backed up"
    );

    // Build the agents/default/ tree, then move the legacy workspace
    // dir into place under that. `rename` is the canonical "atomic"
    // move on the same filesystem; on a cross-fs path (e.g. legacy on
    // tmpfs, target on disk) `rename` fails and we fall back to
    // copy-then-remove.
    std::fs::create_dir_all(agents_dir.join("default")).with_context(|| {
        format!(
            "[system] failed to create per-agent dir {}",
            agents_dir.join("default").display()
        )
    })?;

    if new_default.exists() {
        // Empty target dir from a previous skipped run; remove so
        // rename has a clean slot.
        std::fs::remove_dir(&new_default).with_context(|| {
            format!(
                "[system] failed to remove empty target {} before move",
                new_default.display()
            )
        })?;
    }

    if std::fs::rename(&legacy, &new_default).is_err() {
        // Cross-filesystem path: copy + remove.
        copy_dir_recursive(&legacy, &new_default).with_context(|| {
            format!(
                "[system] failed to copy legacy workspace from {} to {}",
                legacy.display(),
                new_default.display()
            )
        })?;
        std::fs::remove_dir_all(&legacy).with_context(|| {
            format!(
                "[system] failed to remove legacy workspace {} after copy",
                legacy.display()
            )
        })?;
    }

    tracing::info!(
        legacy = %legacy.display(),
        target = %new_default.display(),
        backup = %backup_dir.display(),
        "[system] filesystem migration: legacy workspace moved into default agent slot"
    );
    Ok(true)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_symlink() {
            // Preserve symlinks rather than following them — copying a
            // symlink target out of the workspace would balloon the
            // backup and risk reading-outside-install.
            #[cfg(unix)]
            {
                let target = std::fs::read_link(&from)?;
                std::os::unix::fs::symlink(&target, &to)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&from, &to)?;
            }
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Refuse to proceed if the on-disk config is at a stale schema version.
///
/// Used by CLI write commands (`config set`, `config patch`, `config init`)
/// to ensure the user explicitly opts into the migration via
/// `zeroclaw config migrate` before modifying a stale config — the alternative
/// would be a silent auto-migrate-on-write, which is harder to audit and
/// surprises users who didn't realize their config schema had changed.
///
/// - Missing file → `Ok(())` (fresh install: nothing to migrate yet).
/// - Current version → `Ok(())`.
/// - Stale (or future) version → `Err` with a message that names the disk
///   version and the command the user needs to run.
pub fn ensure_disk_at_current_version(path: &Path) -> Result<()> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::from(e))
                .with_context(|| format!("failed to read config at {}", path.display()));
        }
    };
    let value: toml::Value =
        toml::from_str(&raw).context("failed to parse config TOML for version check")?;
    let from = detect_version(&value)?;
    if from == CURRENT_SCHEMA_VERSION {
        return Ok(());
    }
    if from > CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "config at {} is schema_version {from}, newer than this binary supports ({})",
            path.display(),
            CURRENT_SCHEMA_VERSION,
        );
    }
    anyhow::bail!(
        "config at {} is schema_version {from}; run `zeroclaw config migrate` to update before modifying",
        path.display(),
    );
}

/// Fold a `from_key: String` value into a `to_key: Vec<String>` array on the
/// same table. Used for the singular→plural channel transforms (V1→V2:
/// `matrix.room_id` → `allowed_rooms`, `slack.channel_id` → `channel_ids`;
/// V2→V3: `discord.guild_id` → `guild_ids`, etc.).
///
/// - Removes `from_key` from the table.
/// - If the value was a non-empty string, appends it to `to_key`'s array
///   (creating the array if missing). Existing entries are preserved; the new
///   value is deduplicated against current contents.
/// - Empty strings, non-string types, and missing `from_key` are no-ops.
///
/// Returns `true` if a value was actually folded (caller may emit a log line).
pub(crate) fn fold_string_into_array(
    table: &mut toml::Table,
    from_key: &str,
    to_key: &str,
) -> bool {
    let value = match table.remove(from_key) {
        Some(toml::Value::String(s)) if !s.is_empty() => s,
        Some(other) => {
            // Non-string: re-insert under from_key untouched (caller may handle).
            table.insert(from_key.to_string(), other);
            return false;
        }
        None => return false,
    };
    let entry = table
        .entry(to_key.to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if let Some(arr) = entry.as_array_mut() {
        let already_present = arr.iter().any(|v| v.as_str() == Some(value.as_str()));
        if !already_present {
            arr.push(toml::Value::String(value));
        }
        true
    } else {
        // Existing to_key wasn't an array (unusual). Reinsert from_key as-is.
        table.insert(from_key.to_string(), toml::Value::String(value));
        false
    }
}

/// One typed migration step: `V_n` TOML → `V_{n+1}` TOML.
type MigrationStep = fn(toml::Value) -> Result<toml::Value>;

/// Migration steps keyed 1-indexed by `from` version: `MIGRATION_STEPS[n]`
/// is the step from `V_n` to `V_{n+1}`. Slot 0 is a never-invoked
/// placeholder so callers can write `&MIGRATION_STEPS[from..target]`
/// directly — both bounds read as schema-version numbers, no offset math.
///
/// To add a new schema version `V_n`:
/// 1. Add `schema/v{n-1}.rs` with a partial typed lens for the prior shape.
/// 2. Implement `V{n-1}Config::migrate(self) -> Result<toml::Value>`.
/// 3. Bump [`CURRENT_SCHEMA_VERSION`] to `n`.
/// 4. Append a new closure here that deserializes `V{n-1}Config` and calls
///    its `migrate()`. The compile-time assertion below catches drift.
const MIGRATION_STEPS: &[MigrationStep] = &[
    // V0 → V1: padding so slot 0 is never indexed. V0 does not exist.
    |_| unreachable!("MIGRATION_STEPS[0] is a 1-indexing pad and is never invoked"),
    // V1 → V2
    |value| {
        let v1: V1Config = value
            .try_into()
            .context("failed to deserialize input as V1 schema")?;
        let v2 = v1.migrate();
        toml::Value::try_from(v2).context("failed to serialize V2 intermediate")
    },
    // V2 → V3
    |value| {
        let v2: V2Config = value
            .try_into()
            .context("failed to deserialize as V2 schema")?;
        v2.migrate().context("failed to migrate V2 → V3")
    },
];

const _: () = assert!(
    MIGRATION_STEPS.len() as u32 == CURRENT_SCHEMA_VERSION,
    "MIGRATION_STEPS must have exactly one entry per schema version \
     (length = CURRENT_SCHEMA_VERSION, including the slot-0 padding)",
);

/// Run the typed migration chain from `from` up to `CURRENT_SCHEMA_VERSION`.
/// `from` must be `< CURRENT_SCHEMA_VERSION` (caller checks).
fn run_chain(value: toml::Value, from: u32) -> Result<toml::Value> {
    run_chain_until(value, from, CURRENT_SCHEMA_VERSION)
}

/// Run the typed migration chain from `from` up to `target` (the shape that
/// is emitted). `target` must be in `from..=CURRENT_SCHEMA_VERSION`.
///
/// Used by `migrate_file` / `migrate_to_current` (target = current) and by
/// [`generate`] (target = any historical version, for fixture generation).
fn run_chain_until(value: toml::Value, from: u32, target: u32) -> Result<toml::Value> {
    if target < from {
        anyhow::bail!("cannot migrate backwards from V{from} to V{target}");
    }
    if target > CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "target V{target} exceeds CURRENT_SCHEMA_VERSION (V{CURRENT_SCHEMA_VERSION})"
        );
    }

    let mut cur = value;
    for step in &MIGRATION_STEPS[from as usize..target as usize] {
        cur = step(cur)?;
    }
    Ok(cur)
}

/// Reconcile new typed values into an existing `toml_edit::DocumentMut` so
/// comments and decoration on surviving keys are preserved across save.
///
/// Walks `new` recursively. For each key:
/// - If the key exists in `doc` AND both sides are tables, recurse.
/// - If the key exists in `doc` and at least one side is not a table, replace
///   the value while preserving the key's prefix decor (i.e. the comment lines
///   that lead the key).
/// - If the key does not exist in `doc`, insert it.
///
/// Removed keys (present in `doc` but absent from `new`) are dropped from `doc`.
/// This matches the prior crate behavior: the typed schema is authoritative,
/// and any TOML key not represented in `new` is not part of the current schema.
pub(crate) fn sync_table(doc: &mut toml_edit::Table, new: &toml::Table) {
    // Drop keys not present in new
    let to_remove: Vec<String> = doc
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| !new.contains_key(k))
        .collect();
    for k in to_remove {
        doc.remove(&k);
    }

    for (key, new_value) in new.iter() {
        if let (Some(doc_item), toml::Value::Table(new_sub)) =
            (doc.get_mut(key.as_str()), new_value)
            && let Some(doc_sub) = doc_item.as_table_mut()
        {
            // Both tables — recurse to preserve nested comments.
            sync_table(doc_sub, new_sub);
            continue;
        }
        // Otherwise, replace the value while preserving the key's leading decor.
        let new_item = toml_value_to_edit_item(new_value);
        match doc.get_mut(key.as_str()) {
            Some(existing) => {
                // Preserve the key's leading decor (comments) by mutating in place.
                *existing = new_item;
            }
            None => {
                doc.insert(key.as_str(), new_item);
            }
        }
    }
}

/// Convert a `toml::Value` into a `toml_edit::Item` for insertion into
/// a `DocumentMut`. Tables become inline tables when small, real tables
/// otherwise — matches `toml_edit`'s default round-trip behavior.
fn toml_value_to_edit_item(value: &toml::Value) -> toml_edit::Item {
    // Easiest path: serialize to string, parse as toml_edit. Lossy on numeric
    // formatting nuance but correct for migration round-trip where we're
    // emitting freshly-serialized values.
    let serialized = match value {
        toml::Value::Table(t) => {
            let mut wrapper = toml::Table::new();
            wrapper.insert("__v".into(), toml::Value::Table(t.clone()));
            toml::to_string(&wrapper).unwrap_or_default()
        }
        other => {
            let mut wrapper = toml::Table::new();
            wrapper.insert("__v".into(), other.clone());
            toml::to_string(&wrapper).unwrap_or_default()
        }
    };
    let doc: toml_edit::DocumentMut = serialized.parse().unwrap_or_default();
    doc.get("__v").cloned().unwrap_or(toml_edit::Item::None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_version_missing_is_v1() {
        let v: toml::Value = toml::from_str("foo = 1").unwrap();
        assert_eq!(detect_version(&v).unwrap(), 1);
    }

    #[test]
    fn detect_version_explicit() {
        let v: toml::Value = toml::from_str("schema_version = 2\n").unwrap();
        assert_eq!(detect_version(&v).unwrap(), 2);
    }

    #[test]
    fn detect_version_negative_errors() {
        let v: toml::Value = toml::from_str("schema_version = -1\n").unwrap();
        assert!(detect_version(&v).is_err());
    }

    #[test]
    fn detect_version_string_errors() {
        let v: toml::Value = toml::from_str("schema_version = \"two\"\n").unwrap();
        assert!(detect_version(&v).is_err());
    }

    // ── migrate_file_in_place atomic-write semantics ──

    fn setup_temp_config_dir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("temp dir")
    }

    #[test]
    fn migrate_file_in_place_writes_backup_and_replaces_atomically() {
        let dir = setup_temp_config_dir();
        let path = dir.path().join("config.toml");
        // Minimal V1 input (no schema_version) so migration runs.
        std::fs::write(&path, "default_model_provider = \"openai\"\nfoo = 1\n").unwrap();

        let report = migrate_file_in_place(&path)
            .expect("migration succeeds")
            .expect("migration ran (V1 input)");

        // Backup retains the original content verbatim.
        let backup = std::fs::read_to_string(&report.backup_path).unwrap();
        assert!(
            backup.contains("default_model_provider = \"openai\"") && backup.contains("foo = 1"),
            "backup must contain the original V1 content; got: {backup}"
        );

        // Original is replaced with migrated content.
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(
            migrated.contains("schema_version"),
            "migrated config must carry a schema_version line; got: {migrated}"
        );

        // No `<file>.tmp-*` files left behind in the parent.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".config.toml.tmp-")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp files must remain after a successful migration; got {leftovers:?}"
        );
    }

    #[test]
    fn migrate_file_in_place_noop_when_already_current() {
        let dir = setup_temp_config_dir();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!("schema_version = {CURRENT_SCHEMA_VERSION}\n"),
        )
        .unwrap();

        let report = migrate_file_in_place(&path).expect("idempotent on current schema");
        assert!(
            report.is_none(),
            "no migration should run when the file is already at CURRENT_SCHEMA_VERSION"
        );
        // No backup file should exist when the migration didn't run.
        let backup = path.with_file_name("config.toml.backup");
        assert!(
            !backup.exists(),
            "no `.backup` should be created on the no-op path; got {}",
            backup.display()
        );
    }

    // ── Legacy workspace → per-agent migration tests ─────────────

    #[test]
    fn fs_migration_no_op_on_fresh_install() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path();
        // Fresh install: no `<install>/workspace/`, no
        // `<install>/agents/default/workspace/`. Migration should be
        // a no-op.
        let ran = migrate_legacy_workspace_to_default_agent(install_root).unwrap();
        assert!(!ran, "no legacy dir → no migration runs");
        assert!(
            !install_root.join("agents").join("default").exists(),
            "fresh install should not synthesize the default agent dir from this path"
        );
    }

    #[test]
    fn fs_migration_moves_legacy_workspace_into_default_agent_with_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path();
        let legacy = install_root.join("workspace");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("MEMORY.md"), "# Long-Term Memory\n\nfoo").unwrap();
        std::fs::write(legacy.join("AGENTS.md"), "alpha agent identity").unwrap();

        let ran = migrate_legacy_workspace_to_default_agent(install_root).unwrap();
        assert!(ran, "populated legacy dir → migration runs");

        // Legacy dir is gone.
        assert!(
            !legacy.exists(),
            "legacy workspace must be moved out, not left behind"
        );

        // New target is populated with the legacy contents.
        let new_default = install_root
            .join("agents")
            .join("default")
            .join("workspace");
        assert!(new_default.is_dir(), "target dir created");
        assert_eq!(
            std::fs::read_to_string(new_default.join("MEMORY.md")).unwrap(),
            "# Long-Term Memory\n\nfoo"
        );
        assert_eq!(
            std::fs::read_to_string(new_default.join("AGENTS.md")).unwrap(),
            "alpha agent identity"
        );

        // Backup retains the legacy contents.
        let backups: Vec<_> = std::fs::read_dir(install_root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|s| s.starts_with("backup-"))
            })
            .collect();
        assert_eq!(backups.len(), 1, "exactly one timestamped backup dir");
        let backup_legacy = backups[0].path().join("legacy-workspace");
        assert_eq!(
            std::fs::read_to_string(backup_legacy.join("MEMORY.md")).unwrap(),
            "# Long-Term Memory\n\nfoo"
        );
    }

    #[test]
    fn fs_migration_idempotent_on_already_migrated_install() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path();
        let legacy = install_root.join("workspace");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("MEMORY.md"), "first").unwrap();

        // First run does the work.
        let ran_first = migrate_legacy_workspace_to_default_agent(install_root).unwrap();
        assert!(ran_first);

        // Second run is a no-op (legacy is gone).
        let ran_second = migrate_legacy_workspace_to_default_agent(install_root).unwrap();
        assert!(!ran_second, "no legacy dir → no migration runs");
    }

    #[test]
    fn fs_migration_skips_when_target_already_populated() {
        let tmp = tempfile::tempdir().unwrap();
        let install_root = tmp.path();

        // Both legacy AND new-default exist + populated. The
        // migration must NOT clobber the new dir.
        let legacy = install_root.join("workspace");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("MEMORY.md"), "legacy-content").unwrap();

        let new_default = install_root
            .join("agents")
            .join("default")
            .join("workspace");
        std::fs::create_dir_all(&new_default).unwrap();
        std::fs::write(new_default.join("MEMORY.md"), "new-content").unwrap();

        let ran = migrate_legacy_workspace_to_default_agent(install_root).unwrap();
        assert!(!ran, "populated target → no migration runs");
        assert!(
            legacy.exists(),
            "legacy workspace must be left in place when target is populated"
        );
        assert_eq!(
            std::fs::read_to_string(new_default.join("MEMORY.md")).unwrap(),
            "new-content",
            "target contents must not be clobbered"
        );
    }
}
