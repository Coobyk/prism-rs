# prism-rs

PrismLauncher rewritten in **Rust + [GPUI](https://crates.io/crates/gpui)** (via `gpui-kit`/GPUI Component).

Same UI layout and feature direction as [PrismLauncher](https://github.com/PrismLauncher/PrismLauncher):
a main window with a grouped instance grid, a right-hand instance toolbar, a news bar and status
bar — backed by a from-scratch Rust core.

## Build

```sh
cargo build            # workspace: crates/prism-core + crates/prism-ui
cargo test -p prism-core
```

Linux system deps: `cmake ninja-build libxkbcommon-dev libxkbcommon-x11-dev libfontconfig1-dev
libudev-dev libgl1-mesa-dri` (on headless machines run under `xvfb-run` with `WGPU_BACKEND=gl`).

## Run

```sh
cargo run -p prism-ui --bin prism-rs
```

Data lives in `$PRISM_RS_DATA` (default `~/.local/share/prismlauncher-rs`), laid out like
PrismLauncher's: `instances/`, `icons/`, `cache/{versions,libraries,assets,metadata}`, `java/`,
`accounts.json`, `prismlauncher.cfg`.

Headless CLI (used to verify the launch pipeline without a GUI):

```sh
prism-rs prepare 1.20.1          # fetch metadata, download game files, print the java command
prism-rs import pack.mrpack      # import a Modrinth modpack
prism-rs import pack.zip        # import a CurseForge modpack or MultiMC instance
prism-rs export my-instance out.zip # export Prism zip (or out.mrpack for Modrinth)
```

For automatic CurseForge downloads, set `PRISM_RS_CURSEFORGE_API_KEY` to
your own CurseForge API key before running the CLI or opening the launcher.
If the key is missing or rejected, the instance and overrides still import
and every listed file is reported for manual download.
Files unavailable for automatic download are listed in
`curseforge-missing-files.txt` in the new instance folder and can be added
manually to the appropriate Minecraft folder (`mods`, `resourcepacks`, or `shaderpacks`).

## Architecture

```
crates/prism-core
  ini.rs          instance.cfg / prismlauncher.cfg INI handling
  paths.rs        launcher data directory layout
  instance.rs     instance model, InstanceList, instgroups.json, mmc-pack.json
  settings.rs     global settings with typed defaults
  net.rs          download manager (sha1 validation, concurrency, progress)
  meta.rs         Mojang piston-meta version JSON: parsing, rules, gradle specifiers
  profile.rs      resolved LaunchProfile (libraries, natives, args) + loader overlay
  loaders.rs      Fabric/Quilt meta APIs; Forge/NeoForge via official installers
  launch.rs       launch pipeline: fetch → download → natives → java command
  java.rs         Java detection and version parsing
  auth.rs         accounts.json, offline accounts, MSA device-code login (XBL→XSTS→MC)
  modplatform/    Modrinth (search/versions/install/.mrpack) and CurseForge (Flame)
  resources.rs    local mod/pack scanning (fabric.mod.json, mods.toml, mcmod.info, quilt)
  import.rs       instance import: .mrpack, CurseForge modpacks, MultiMC zips
  export.rs       Prism zip and Modrinth mrpack export (hash resolution + overrides)
  news.rs         Atom/RSS news feed

crates/prism-ui (GPUI)
  main_window.rs      main window: toolbar, grouped instance grid, sidebar, console, news/status
  instance_window.rs  per-instance pages: Log, Version (loader install), Mods, Browse Mods
                      (Modrinth search + install), Resource/Shader packs, Worlds, Screenshots,
                      Notes, Settings
  accounts_window.rs  account list, offline accounts, MSA device-code sign-in
  settings_window.rs  global settings (memory, window size, Java)
  dialogs.rs          create-instance, rename, change-group, import dialogs
```

## Status

Working today:

- Launch Minecraft (vanilla, Fabric, Quilt) — verified by booting MC 1.20.1 headlessly
- Instance create / rename / group / delete (trash) / copy of directories
- Version manifest browsing, loader install (Fabric/Quilt from meta APIs, Forge/NeoForge via
  installer)
- Modrinth: search, install mods, import `.mrpack` modpacks
- CurseForge: import modpack zips
- Offline accounts; MSA device-code login implemented end-to-end
- Global + per-instance settings (memory, Java, window size)
- Live console with log capture; kill running instances
- News feed, grouped instance list, persistent config
- Instance export: Prism zip and Modrinth mrpack (CLI and instance toolbar)

Not yet at parity (tracked in the issue tracker of your heart): ATL/FTB/Technic pack
import, CurseForge export, servers.dat NBT editing, screenshots
upload, themes/translations, updater, jar mods, profiling tools, per-instance console window
options, quick-play singleplayer/multiplayer dialogs.

## License

MIT (see LICENSE). The original PrismLauncher is GPL-3.0; this is a clean-room reimplementation in
a different language, following its feature set and file formats for interoperability.
