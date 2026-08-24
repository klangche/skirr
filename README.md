# skirr

Skírr – Old Norse for *clear, bright, pure*. A USB diagnostic tool for ProAV
systems. It maps the full device tree — hubs, ports, speeds and displays —
then shows exactly why a connection fails. No guessing: a clear verdict and
the facts behind it.

## Status

MVP feature-complete on **macOS** (Apple Silicon live-tested) and **Windows**
(code-complete, runner review pending). Linux and the GUI are on the roadmap —
see [SKIRR_PLANNER.md](SKIRR_PLANNER.md).

## Build

```sh
cargo build --release -p skirr-cli
./target/release/skirr --help
```

## Usage

```sh
skirr scan       # platform info + every USB device
skirr topology   # tree with hops/tiers
skirr hubs       # hub port mapping
skirr ports      # occupied ports
skirr diagnose   # rule-engine verdict (exit 1 = FAIL findings)
skirr monitor    # live hotplug events (Ctrl-C to stop)
skirr report     # JSON report of the current state
```

Global flags: `--json` for machine-readable output, `--profile <FILE>` for a
custom rule profile (defaults to Skirr Standard Profile v1.0).

## First launch: Gatekeeper & SmartScreen

Prebuilt binaries are currently **unsigned**, so the OS asks for confirmation
exactly once per app.

### macOS

1. Drag `Skirr.app` from the DMG into **Applications**.
2. On first launch you may see *"Skirr can't be opened because Apple cannot
   check it for malicious software."* Right-click `Skirr.app` → **Open** →
   **Open** again to confirm.
3. Alternatively use **System Settings → Privacy & Security** and click
   **Open Anyway** at the bottom.
4. CLI-only alternative, no prompts:
   ```sh
   /Applications/Skirr.app/Contents/MacOS/skirr scan
   ```
   If the binary was quarantined after download, clear the flag first:
   ```sh
   xattr -d com.apple.quarantine /Applications/Skirr.app
   ```

### Windows (SmartScreen)

1. Run `skirr.exe`. Windows SmartScreen shows *"Windows protected your PC."*
2. Click **More info** → **Run anyway**.
3. The prompt appears once; afterwards the exe starts normally.

Both flows only appear because builds are not notarized/signed yet. Once code
signing certificates are in place this section shrinks to nothing.

## Releases

Tagging `v*` triggers [.github/workflows/release.yml](.github/workflows/release.yml),
which builds and attaches:

| OS | Arch | Artifact |
|----|------|----------|
| Windows | x64 | portable `.zip` (`skirr.exe`) |
| macOS | ARM64 (Apple Silicon) | `.dmg` (`Skirr.app`) |
| macOS | x64 (Intel) | `.dmg` (`Skirr.app`) |
| Ubuntu 22.04 | x86_64 | `.deb` (`/usr/bin/skirr`) |

Each artifact ships with a SHA-256 checksum file.

## Documentation

- [docs/DATA_MAP.md](docs/DATA_MAP.md) — data sources per platform, field mapping
- [SKIRR_PLANNER.md](SKIRR_PLANNER.md) — roadmap and progress

## License

MIT
