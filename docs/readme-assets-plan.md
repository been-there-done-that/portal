# README Assets Plan

Date: 2026-04-08

## Goal

Produce a small, high-quality visual package for the top-level README so Portal looks intentional and current instead of text-only.

## Asset Set

### 1. `docs/assets/readme/hero-terminal.png`

Purpose:

- primary hero visual
- shows the portal CLI payoff in one glance

Shot should include:

- `portal start` or `portal run npm run dev`
- setup output
- final `https://myapp.localhost`
- a realistic shell prompt and project context

Style:

- dark terminal
- crisp monospace rendering
- one accent color
- no exaggerated fake glow

### 2. `docs/assets/readme/before-after.png`

Purpose:

- communicate the product shift instantly

Layout:

- left: messy local ports
- right: named local URLs

Copy:

- before: `localhost:3000`, `localhost:5173`, `localhost:8080`
- after: `web.localhost`, `admin.localhost`, `api.localhost`

### 3. `docs/assets/readme/architecture-overview.svg`

Purpose:

- replace or complement Mermaid near the hero/docs boundary

Must show:

- browser
- portal CLI
- portal daemon
- route store
- TLS cert resolver
- local app

### 4. `docs/assets/readme/request-inspector.png`

Purpose:

- second proof asset if inspector is stable enough to show publicly

Shot should include:

- request list
- one selected request
- method, path, status, headers/body panes

Rule:

- do not include in the shipped README until the runtime path is verifiably wired

### 5. `docs/assets/readme/run-lifecycle.svg`

Purpose:

- turn the `portal run` sequence into a polished diagram for docs/social reuse

## Visual Direction

- terminal-native
- dark neutral background
- white and gray type
- cyan or vivid blue accent
- minimal gradients
- no purple-heavy generic SaaS styling

## Production Order

1. hero terminal image
2. architecture overview SVG
3. before/after image
4. inspector screenshot
5. lifecycle SVG

## Acceptance Criteria

- each asset is readable inside GitHub markdown width
- text is still legible on laptop screens
- visuals support the README, not overwhelm it
- no asset claims a feature that is not actually runnable
