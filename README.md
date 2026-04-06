# portal

Replace `localhost:3000` with `https://myapp.localhost` — real URLs for local dev.

```
portal run npm run dev
# → https://myapp.localhost
```

---

## What it does

Portal assigns a clean `.localhost` HTTPS URL to any dev server. No more remembering port numbers, no more `http://`. Works with any framework — just wrap your start command.

```
portal run npm run dev          # → https://myapp.localhost
portal run yarn start           # → https://myapp.localhost
portal run python manage.py runserver  # → https://myapp.localhost
portal run --hostname api npm start    # → https://api.localhost
```

---

## How it works

```mermaid
graph LR
    Browser -->|"https://myapp.localhost:443"| Daemon
    Daemon -->|SNI lookup| CertResolver
    CertResolver -->|"on-demand TLS cert\n(cached)"| Daemon
    Daemon -->|"http://127.0.0.1:PORT"| DevServer

    subgraph Portal Daemon [portal daemon — runs as root]
        Daemon[HTTPS Proxy\nport 443]
        CertResolver[Cert Resolver\nrcgen + rustls]
    end

    subgraph Your Machine
        DevServer[Your App\nlocalhost:PORT]
    end
```

```mermaid
sequenceDiagram
    participant You
    participant CLI as portal CLI
    participant Daemon as portal daemon
    participant App as Your App

    You->>CLI: portal run npm run dev
    CLI->>Daemon: ensure running (auto-start if needed)
    CLI->>App: spawn process with PORT=auto
    App-->>CLI: listening on port 4072
    CLI->>Daemon: RegisterRoute {hostname, port, pid}
    Daemon-->>CLI: ok
    CLI->>You: https://myapp.localhost → port 4072

    Note over You,App: Dev server is now live

    You->>Daemon: GET https://myapp.localhost/api/users
    Daemon->>Daemon: TLS handshake (auto-cert for myapp.localhost)
    Daemon->>App: GET http://127.0.0.1:4072/api/users
    App-->>Daemon: 200 OK
    Daemon-->>You: 200 OK (HTTPS)
```

---

## Architecture

```mermaid
graph TD
    subgraph Binary ["portal binary (single binary, two roles)"]
        CLI[CLI Mode\nThin IPC client]
        Daemon[Daemon Mode\nPORTAL_IS_DAEMON=1]
    end

    CLI -->|"Unix socket\nlength-prefixed JSON frames"| IPC[IPC Server]

    subgraph Daemon
        IPC --> RouteStore[Route Store\nDashMap + JSON]
        IPC --> CertStore[Cert Store\nCA + per-host certs]
        HTTPListener[HTTP :80\nRedirect to HTTPS] 
        HTTPSListener[HTTPS :443\nTLS Proxy]
        HTTPSListener --> CertStore
        HTTPSListener --> RouteStore
    end

    CLI --> SpawnChild[Spawn Child Process\nPORT=auto injected]
    SpawnChild -->|pid + port| IPC
```

### IPC Protocol

All CLI→daemon communication uses a Unix socket at `~/.portal/portal.sock` with length-prefixed JSON frames:

```
[4-byte big-endian u32 length][UTF-8 JSON payload]
```

Commands: `ls`, `status`, `run`, `stop`, `rm`, `shutdown`, `cert_install`, `cert_reset`, `register_route`

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/been-there-done-that/portal/main/install.sh | bash
```

Or build from source:

```bash
cargo install --git https://github.com/been-there-done-that/portal
```

### First-time setup

Install the local CA into your system trust store (needed once):

```bash
sudo portal cert install
```

Then restart your browser. All `.localhost` URLs will show the padlock.

### Privileged ports (recommended)

Running on ports 80/443 gives you clean `https://myapp.localhost` URLs without a port number:

```bash
sudo portal daemon
```

For non-root use, portal defaults to `8080`/`4443`. Configure in `~/.portal/config.toml`:

```toml
[proxy]
http_port = 8080
https_port = 4443
```

---

## Usage

```
portal <command> [options]
```

| Command | Description |
|---------|-------------|
| `portal run <cmd>` | Start a dev server and assign it a `.localhost` URL |
| `portal run --force <cmd>` | Kill any existing instance first, then start |
| `portal run --hostname <name> <cmd>` | Use a specific hostname |
| `portal ls` | List all active routes |
| `portal stop <hostname>` | Stop a running server |
| `portal status` | Show daemon info |
| `portal daemon` | Start the background daemon |
| `portal shutdown` | Stop the daemon |
| `portal cert install` | Install the local CA |
| `portal cert reset` | Regenerate and reinstall the CA |

### Hostname detection

Portal picks the hostname automatically from the project directory:

```mermaid
flowchart TD
    A[portal run] --> B{--hostname flag?}
    B -->|yes| Z[Use it]
    B -->|no| C{portal.toml name?}
    C -->|yes| Z
    C -->|no| D{Git worktree?}
    D -->|yes| E["branch-dirname.localhost\ne.g. feat-auth-myapp.localhost"]
    D -->|no| F["dirname.localhost\ne.g. myapp.localhost"]
    E --> G[sanitize: lowercase, non-alphanumeric → dash]
    F --> G
```

### Framework port injection

Portal detects the framework and injects the port the right way — no config needed:

```mermaid
flowchart LR
    A[portal run] --> B{Detect framework}
    B -->|Vite / Astro / Nuxt| C["--port PORT"]
    B -->|Create React App| D["--port PORT"]  
    B -->|Angular| E["--port PORT"]
    B -->|Remix / React Router| F["--port PORT"]
    B -->|Expo| G["--port PORT"]
    B -->|anything else| H["PORT=PORT env var"]
```

---

## Configuration

### `~/.portal/config.toml` — global

```toml
[proxy]
tld = "localhost"           # TLD for generated hostnames
http_port = 80              # HTTP redirect listener
https_port = 443            # HTTPS proxy listener
https = true                # Enable HTTPS
port_range = [4000, 4999]   # Range for auto-assigned backend ports

[daemon]
log_level = "info"
auto_start = true
```

### `portal.toml` — per-project

```toml
[project]
name = "my-api"             # Overrides auto-detected hostname
```

### Environment variables

| Variable | Description |
|----------|-------------|
| `PORTAL_TLD` | Override TLD |
| `PORTAL_HTTPS` | `1`/`true`/`yes`/`on` to enable HTTPS |
| `PORTAL_HTTP_PORT` | Override HTTP port |
| `PORTAL_HTTPS_PORT` | Override HTTPS port |
| `PORTAL_LOG` | Log level (`debug`, `info`, `warn`, `error`) |

---

## Certificate lifecycle

```mermaid
stateDiagram-v2
    [*] --> NoCerts: first run
    NoCerts --> CAGenerated: portal daemon starts\n(rcgen, 10-year validity)
    CAGenerated --> CAInstalled: portal cert install\n(macOS Keychain / system store)
    CAInstalled --> HostCertGenerated: first request to myapp.localhost\n(on-demand via SNI callback)
    HostCertGenerated --> HostCertCached: cached in DashMap\n(reused for all subsequent requests)
    HostCertCached --> HostCertGenerated: new hostname
    CAInstalled --> CARegenerated: portal cert reset
    CARegenerated --> CAInstalled
```

The CA key lives at `~/.portal/certs/ca-key.pem` (mode `0600`). Per-host certs are generated on first request and kept in memory — no disk writes per hostname.

---

## Daemon lifecycle

```mermaid
stateDiagram-v2
    [*] --> Stopped
    Stopped --> Starting: portal daemon\n(or auto-start on portal run)
    Starting --> Running: bound ports 80 + 443\nIPC socket ready
    Running --> Running: serving requests\nregistering routes
    Running --> Stopped: portal shutdown\n(removes socket + pid file)
    Running --> Stopped: process killed
```

The daemon uses a **re-spawn approach** (not double-fork) to avoid fork-in-async-runtime issues. The CLI spawns itself with `PORTAL_IS_DAEMON=1` and exits — the child becomes the daemon.

---

## Development

```bash
# Build
cargo build --release

# Run tests
cargo test

# Install locally
cargo install --path .

# Run daemon in foreground (for debugging)
PORTAL_IS_DAEMON=1 PORTAL_LOG=debug portal daemon
```

---

## License

Apache-2.0
