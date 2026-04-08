# Group B-A — Storybook Driver Design

**Goal:** Add a `StorybookDriver` that auto-detects Storybook projects and assigns them a `{project}-storybook.localhost` URL with correct port injection.

**Architecture:** One new file `src/detect/storybook.rs`, registered in `DriverRegistry` at priority 45 (above NodeDriver's 40). No changes to daemon, proxy, or CLI.

---

## Detection

`StorybookDriver::detect(cwd)` returns `true` when any of the following are true:

1. `.storybook/` directory exists in `cwd`
2. `package.json` `scripts` object contains a `"storybook"` or `"start-storybook"` key
3. `package.json` `devDependencies` contains any key starting with `"@storybook/"`

All three checks are file-local (no network, no subprocess).

## Priority

**45** — above NodeDriver (40) so a project with both `package.json` and `.storybook/` is recognised as Storybook rather than a generic Node project.

## Start Command

Check `package.json` scripts in order:

1. `"storybook"` key present → `{pm} run storybook`
2. `"start-storybook"` key present → `{pm} run start-storybook`
3. Neither found → `storybook dev` (assumes global/npx install)

Package manager resolved from lock files in `cwd` (same logic as NodeDriver):
- `pnpm-lock.yaml` → `pnpm`
- `bun.lockb` or `bun.lock` → `bun`
- `yarn.lock` → `yarn`
- Otherwise → `npm`

## Port Injection

`CliArgs(vec!["--port".to_string(), port.to_string()])`

Storybook's CLI accepts `--port <n>` directly. The host binding defaults to `localhost` in Storybook 7+ which is sufficient for portal's loopback proxy.

## Hostname (`project_name`)

1. Read `name` field from `package.json` → strip to slug → append `-storybook`
2. Fallback: `{cwd directory name}-storybook`

Examples:
- `package.json` name `"my-app"` → `my-app-storybook` → `my-app-storybook.localhost`
- No `package.json` name, dir is `frontend` → `frontend-storybook.localhost`

User can override with `portal run --hostname custom-name` or `name = "..."` in `portal.toml`.

## Files Changed

| File | Change |
|---|---|
| `src/detect/storybook.rs` | New file — `StorybookDriver` struct and `LanguageDriver` impl |
| `src/detect/mod.rs` | Add `mod storybook;`, push `StorybookDriver` into `DriverRegistry::new()` |

## Testing

- `detect()` returns true for each of the three detection signals independently
- `detect()` returns false when none apply
- `start_command()` picks correct script name and package manager
- `start_command()` falls back to `storybook dev` when no matching script
- `project_name()` appends `-storybook` to package name and directory fallback
- `port_injection()` returns `CliArgs(["--port", "<port>"])`
