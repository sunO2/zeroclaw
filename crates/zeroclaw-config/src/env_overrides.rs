//! V0.8.0 env-var override mechanism.
//!
//! Grammar: `ZEROCLAW_<dotted_path_with_double_underscores>=<value>`.
//! Each `__` (double underscore) is a path separator (`.` in the TOML); each
//! single `_` is either a snake-case joiner inside a field name (which the
//! walker converts to kebab `-` for `set_prop`) or a literal char inside an
//! alias key.
//!
//! Schema-derived: [`map_key_sections`] gives HashMap positions (one alias
//! token consumed; alias chars are `[a-z0-9_]`); [`prop_fields`] gives every
//! other leaf path. No string-literal pattern matching, no hardcoded family
//! names.
//!
//! Bootstrap exception: `ZEROCLAW_WORKSPACE` and `ZEROCLAW_CONFIG_DIR` keep
//! their UPPERCASE form. The case rule (lowercase tail = config-tree,
//! uppercase tail = bootstrap) does the disambiguation work without an
//! exemption list.
//!
//! [`map_key_sections`]: crate::schema::Config::map_key_sections
//! [`prop_fields`]: crate::schema::Config::prop_fields

use crate::schema::Config;
use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::sync::LazyLock;

const PREFIX: &str = "ZEROCLAW_";
const SEP: &str = "__";

/// Paths that the schema exposes via `prop_fields()` but that operators must
/// not override at runtime. Currently just `schema-version` (kebab form, as
/// emitted by `prop_fields()`) — the migration engine sets it from the
/// on-disk file's value, and an env override would either skip needed
/// migrations or trigger a no-op rerun. O(1) HashSet lookup so adding more
/// reserved paths stays cheap.
static NON_OVERRIDABLE_PATHS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["schema-version"]));

/// Apply every `ZEROCLAW_<lowercase>` env var to `config`. Returns the set of
/// dotted prop-paths that were overridden so `save()` can mask them and
/// display layers can render the 💉 indicator via O(1) `HashSet::contains`.
/// Hard-errors on any env var that doesn't resolve to a known schema path.
pub fn apply_env_overrides(config: &mut Config) -> Result<HashSet<String>> {
    let mut entries: Vec<(String, String, String)> = std::env::vars()
        .filter_map(|(k, v)| {
            let tail = k.strip_prefix(PREFIX)?;
            (!tail.is_empty()
                && tail
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
            .then(|| (k.clone(), v, tail.to_string()))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut overridden: HashSet<String> = HashSet::with_capacity(entries.len());
    for (env_name, value, tail) in entries {
        let path = resolve_path(&tail, config)
            .with_context(|| format!("{env_name} did not resolve to a schema path"))?;
        if NON_OVERRIDABLE_PATHS.contains(path.as_str()) {
            return Err(anyhow!(
                "{env_name} → {path}: this field is not overridable via env vars",
            ));
        }
        config
            .set_prop(&path, &value)
            .with_context(|| format!("{env_name} → {path}"))?;
        if Config::prop_is_secret(&path) {
            tracing::warn!(path = %path, env_var = %env_name, "Secret applied from env override");
        } else {
            tracing::debug!(path = %path, env_var = %env_name, "Env override applied");
        }
        overridden.insert(path);
    }
    if !overridden.is_empty() {
        tracing::info!(count = overridden.len(), "Applied env-var config overrides");
    }
    Ok(overridden)
}

/// Walk an env-var tail against the schema. Map-keyed positions consume one
/// `__`-delimited alias token (which may contain single `_` per the alias
/// validator); everything else resolves via `prop_fields()` lookup.
fn resolve_path(tail: &str, config: &mut Config) -> Result<String> {
    let mut sections = Config::map_key_sections();
    sections.sort_by_key(|s| std::cmp::Reverse(s.path.len()));
    for section in sections {
        let env_pfx: String = section.path.replace('.', SEP);
        let with_sep = format!("{env_pfx}{SEP}");
        let Some(rest) = tail.strip_prefix(&with_sep) else {
            continue;
        };
        let mut parts = rest.splitn(2, SEP);
        let alias = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing alias after `{}`", section.path))?;
        let inner = parts.next().unwrap_or("");
        let _ = config.create_map_key(section.path, alias);
        let path = if inner.is_empty() {
            format!("{}.{}", section.path, alias)
        } else {
            // Inner segments are `__`-separated; each segment is a snake-case
            // field name that maps to kebab in the prop-path.
            let inner_path = inner
                .split(SEP)
                .map(|seg| seg.replace('_', "-"))
                .collect::<Vec<_>>()
                .join(".");
            format!("{}.{}.{}", section.path, alias, inner_path)
        };
        return Ok(path);
    }

    // Non-map path: prop_fields() entries are dotted with kebab fields.
    // Convert to env-form (`.` → `__`, `-` → `_`) and compare.
    config
        .prop_fields()
        .into_iter()
        .find(|f| f.name.replace('.', SEP).replace('-', "_") == tail)
        .map(|f| f.name)
        .ok_or_else(|| anyhow!("no schema field has env-form `{tail}`"))
}

/// Mask env-overridden paths in a save-bound clone so env-injected values
/// never reach `encrypt_secrets()` or the on-disk TOML.
pub fn mask_env_overrides_for_save(
    config_to_save: &mut Config,
    disk: Option<&Config>,
    paths: &HashSet<String>,
) -> Result<()> {
    for path in paths {
        let value = disk.and_then(|d| d.get_prop(path).ok()).unwrap_or_default();
        if let Err(err) = config_to_save.set_prop(path, &value) {
            tracing::warn!(path = %path, error = %err, "Save-mask reset failed; field retains default");
        }
    }
    Ok(())
}
