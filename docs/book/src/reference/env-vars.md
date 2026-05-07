# Environment Variables

V0.8.0 consolidated every operator env-var override into a single schema-mirror grammar. The tail of a `ZEROCLAW_*` env var is the dotted prop-path that `zeroclaw config set` accepts, with each `__` (double underscore) separating path segments and each single `_` either a snake-case joiner inside a field name (`api_key` → `api-key` in `set_prop`) or a literal char inside an alias key.

```sh
ZEROCLAW_<dotted_path_with_double_underscores>=<value>
```

## Examples

```sh
# Inject a typed-family alias credential
ZEROCLAW_providers__models__anthropic__default__api_key=sk-ant-...

# Set a model on a non-default OpenRouter alias (alias with underscore is fine)
ZEROCLAW_providers__models__openrouter__prod_v2__model=anthropic/claude-sonnet-4-6
ZEROCLAW_providers__models__openrouter__prod_v2__api_key=sk-or-...

# Toggle and configure a channel
ZEROCLAW_channels__matrix__enabled=true
ZEROCLAW_channels__matrix__homeserver=https://matrix.example.org

# Override gateway runtime knobs
ZEROCLAW_gateway__request_timeout_secs=120
ZEROCLAW_gateway__long_running_request_timeout_secs=900

# Inject webhook signing secrets
ZEROCLAW_channels__whatsapp__default__app_secret=...
ZEROCLAW_channels__linq__default__signing_secret=...
ZEROCLAW_channels__nextcloud_talk__default__webhook_secret=...

# Inject Qdrant memory backend connection
ZEROCLAW_storage__qdrant__default__url=https://qdrant.example.com
ZEROCLAW_storage__qdrant__default__collection=zeroclaw
ZEROCLAW_storage__qdrant__default__api_key=...
```

The mapping from env-var name to TOML path is mechanical:

| TOML | Env var |
|---|---|
| `[providers.models.anthropic.default] api_key = "..."` | `ZEROCLAW_providers__models__anthropic__default__api_key=...` |
| `[channels.matrix] homeserver = "..."` | `ZEROCLAW_channels__matrix__homeserver=...` |
| `[gateway] request_timeout_secs = 120` | `ZEROCLAW_gateway__request_timeout_secs=120` |

## Bootstrap (uppercase tail)

Two env vars decide *where* the config file lives, before any `Config` exists. They keep their UPPERCASE form so the case rule disambiguates them from the schema-mirror surface:

```sh
ZEROCLAW_WORKSPACE=/srv/zeroclaw          # workspace root
ZEROCLAW_CONFIG_DIR=/etc/zeroclaw         # config-file location
```

## Persistence boundary

Values applied via `ZEROCLAW_*` env vars land on the **in-memory** `Config` at load time and are **never** persisted to disk. `zeroclaw config save` masks env-overridden paths back to their disk-or-default values before encryption. A `WARN` log line is emitted whenever a secret-typed path (e.g. an API key) is env-overridden, so audit logs make the injection visible.

## Alias grammar

Aliases (the `<alias>` segments above — e.g. `default`, `prod_v2`, `mymatrixalias`) follow these rules:

- lowercase ASCII letters, digits, and single underscores
- must start AND end with a letter or digit (no leading or trailing underscore)
- no `__` substring (reserved as the env-var grammar's path separator)
- no hyphen (illegal in env-var identifiers)
- no uppercase (would conflict with bootstrap names)
- 1–63 characters

`prod_v2` is a single alias token; `default__api_key` parses as two segments (alias `default`, field `api_key`). Configs with non-conforming aliases produce a load-time error naming the offending alias.

## Errors

Unresolvable `ZEROCLAW_<lowercase_*>` names (typos, paths that don't match any prop in the schema) abort startup with a hard error naming the offending env var. Pre-V0.8.0 env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) have no `ZEROCLAW_` prefix, so the override layer never sees them — they're silently ignored at runtime. Operators must migrate to the new grammar.

## Visibility

The override state is surfaced wherever the config is rendered, with a 💉 indicator marking env-overridden fields:

- **`zeroclaw config list`** — legend `💉 env-overridden  🔒 secret` printed once at the top; rows for env-overridden fields are prefixed with 💉.
- **Web Config editor** — every `ListEntry` carries an `is_env_overridden` bool. Env-overridden field rows render the 💉 badge and a persistent warning *"Edits here won't take effect — overridden by ZEROCLAW_..."* so operators see the override without having to attempt an edit.
- **CLI/TUI onboarding** — `prompt_field` skips env-overridden fields and prints a 💉 three-line note (the env var name, the TOML path, and a skip notice) that clears on next/back navigation. Operators don't get prompted to type a value they've already injected.
- **Programmatic** — `Config::prop_is_env_overridden(path) -> bool` is an O(1) HashSet lookup. Hooks here for any custom render layer.

## Migration from earlier versions

Every legacy env-var override has been eradicated. The replacement is the schema-mirror grammar above. Three steps to derive the new name from any TOML key:

1. **Prefix the path with `ZEROCLAW_`.** The dotted TOML path is the source of truth — find the field in your `config.toml` (or in `zeroclaw config schema`).
2. **Replace `.` with `__`** (double underscore — the path separator).
3. **Field name stays as-is** (snake_case). Aliases stay as-is. Nothing else transforms.

For example, `[providers.models.anthropic.default] api_key = "sk-..."` lives at the dotted path `providers.models.anthropic.default.api_key`. Apply the three rules and the env var is `ZEROCLAW_providers__models__anthropic__default__api_key=sk-...`. Same mechanical mapping for any field in any section.

A representative sample of legacy → new mappings:

| Pre-V0.8.0 | V0.8.0 |
|---|---|
| `ANTHROPIC_API_KEY=sk-ant-...` | `ZEROCLAW_providers__models__anthropic__default__api_key=sk-ant-...` |
| `OPENAI_API_KEY=sk-...` | `ZEROCLAW_providers__models__openai__default__api_key=sk-...` |
| `ZEROCLAW_GATEWAY_TIMEOUT_SECS=120` | `ZEROCLAW_gateway__request_timeout_secs=120` |
| `ZEROCLAW_WHATSAPP_APP_SECRET=...` | `ZEROCLAW_channels__whatsapp__default__app_secret=...` |
| `LINE_CHANNEL_ACCESS_TOKEN=...` | `ZEROCLAW_channels__line__default__channel_access_token=...` |
| `QDRANT_URL=...` | `ZEROCLAW_storage__qdrant__default__url=...` |

For every `<UPPER_FAMILY>_API_KEY` that previously worked (Bedrock, Mistral, Groq, DeepSeek, xAI, Together, Fireworks, Novita, Perplexity, Cohere, Moonshot, GLM, Z.AI, Qianfan, Doubao, Qwen/Dashscope, NVIDIA, Synthetic, OpenCode, Vercel, Cloudflare, OVH, Astrai, Avian, DeepMyst, LlamaCPP, SGLang, vLLM, Aihubmix, SiliconFlow, Osaurus, Telnyx, Azure): apply the same three rules to the typed-family alias path (`providers.models.<family>.<alias>.api_key`).

Special cases:

- `ZEROCLAW_API_KEY` / `API_KEY` (generic credential fallbacks): no longer read; pick a typed-family alias path.
- `MINIMAX_OAUTH_*` (auto-refresh flow): the in-process refresh flow was deleted. Obtain access tokens externally and inject via `ZEROCLAW_providers__models__minimax__<alias>__api_key=<access_token>`.
- `QWEN_OAUTH_*` (env-var fallback): the Qwen OAuth file cache at `~/.qwen/oauth_creds.json` (populated by `qwen login`) survives; or set api_key directly via the schema-mirror grammar.
- `GEMINI_OAUTH_CLIENT_ID/SECRET`: read from the cached credentials file populated by `gemini login`. Direct env-var injection was removed.
- `KILO_CLI_PATH` / `GEMINI_CLI_PATH`: replaced by the typed `binary_path` field on `[providers.models.kilocli.<alias>]` and `[providers.models.gemini_cli.<alias>]`. Inject via `ZEROCLAW_providers__models__{kilocli,gemini_cli}__<alias>__binary_path=/path/to/bin`.
- `ZEROCLAW_LUCID_*` (memory backend tunables): defaults only; re-introduce as schema fields if operator demand surfaces.
- `ZEROCLAW_CODEX_*` (URL / reasoning effort overrides): URL flows through the typed alias's `uri`; reasoning effort through `runtime.reasoning_effort`.
- `ZEROCLAW_PROVIDER` / `PROVIDER` / `ZEROCLAW_MODEL` (V1/V2 dispatchers): gone since PR #6266; pick a typed-family alias.
