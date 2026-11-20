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
prism-rs instances              # list installed instances with their IDs
prism-rs launch my-instance      # launch an installed instance without the launcher window
prism-rs import pack.mrpack      # import a Modrinth modpack
prism-rs import pack.zip        # import a CurseForge modpack or MultiMC instance
prism-rs export my-instance out.zip # export Prism zip (or out.mrpack for Modrinth)
```

On Linux, select an instance and click **Create Shortcut** to add a `.desktop` launcher to
your desktop. It uses the current launcher binary and data directory, so recreate the
shortcut if either moves. Some desktop environments ask you to trust a new shortcut before
opening it. A terminal shows game output and stays open on launch or cleanup failure.
Redirect stdout and stderr from any background hook job that should outlive the hook.
The GUI waits for captured output to close; CLI launches pass hook output to the terminal.

For automatic CurseForge downloads, set `PRISM_RS_CURSEFORGE_API_KEY` to
your own CurseForge API key before running the CLI or opening the launcher.
If the key is missing or rejected, the instance and overrides still import
and every listed file is reported for manual download.
Files unavailable for automatic download are listed in
`curseforge-missing-files.txt` in the new instance folder and can be added
manually to the appropriate Minecraft folder (`mods`, `resourcepacks`, or `shaderpacks`).

To export a CurseForge modpack, select a Java instance and choose **Export →
CurseForge zip**. Choose a destination, pack name, version, and optional author.
The launcher looks up fingerprints with the key from **Settings → API Keys** or
`PRISM_RS_CURSEFORGE_API_KEY`. Available exact matches are listed in
`manifest.json`; unmatched files, disabled mods, and other game files are bundled
under `overrides/`. If lookup is needed but fails, no archive is written.

With that same key, open a Java instance with a mod loader and go to **Mods → Browse
CurseForge**. Search by name or leave the field blank to browse. **Install** downloads
the newest compatible, downloadable release for that project (falling back to beta
or alpha if needed), verifies its checksum, and saves metadata for future updates.
Install required dependencies separately.

To find packs for a Java instance, open **Resource Packs** or **Shader Packs** and choose
**Browse Modrinth**. Search by name or leave the field blank to browse, then click
**Install** to download a compatible file for that instance's Minecraft version.
**Back to installed** shows the downloaded pack. Select it in Minecraft to use it;
shader packs still need a shader loader such as Iris.

Open **Worlds** on a Java instance to see each save's name, folder, seed, game
mode, and last-played time. Join a world, open its folder, or refresh the list.
Stop Minecraft before copying, renaming, or deleting a world. Rename changes
the name shown in Minecraft and gives the save folder a matching, unique name.

Use **Minecraft Log** to find and highlight text in the console, copy the full log,
or share it on mclo.gs. Upload asks for confirmation and redacts known usernames,
home paths, and tokens; review the log for other private information before sharing.
**Other Logs** browses the Java instance's `logs/` folder, including compressed
`.log.gz` files. Select a file to view, search, copy, or share it. Stop Minecraft
before deleting logs; deletion requires confirmation.

Open **Screenshots** to browse Minecraft PNGs as thumbnails. Select one to rename
it, delete it with confirmation, or share it on Imgur. To enable sharing, start
prism-rs with your own Imgur application client ID in `PRISM_RS_IMGUR_CLIENT_ID`.
Each upload asks for confirmation before sending the selected screenshot.

Open **Data Packs** on a Java instance and select a saved world. Use **Add ZIP…** for
an archive with `pack.mcmeta` at its root, or use **Browse Modrinth** to install a
compatible data pack. Enable, disable, or remove installed packs and open that
world's `datapacks` folder from the same page. Disabled unpacked packs are stored
in the world's sibling `datapacks.disabled` folder. Stop Minecraft before editing.

For mod updates, open a Java instance with a mod loader and select **Mods → Check for Updates**.
Review and apply compatible updates one at a time; close Minecraft before applying them.
Modrinth mods are identified by file hash or packwiz metadata. CurseForge mods require
packwiz metadata and `PRISM_RS_CURSEFORGE_API_KEY`. Mods installed through Browse Mods
or downloaded during CurseForge import save this metadata in `mods/.index`.

### Horizon

Install the [official Horizon launcher](https://playhorizon.dev/download). Put
`horizon-launcher` on your PATH or set **Horizon launcher path** in Prism's
global settings. In **Add Instance**, choose **Horizon** to create a Horizon
instance, then select it and click **Launch**.

Prism creates a hidden `.prism-horizon-link` file inside the Horizon instance's
game folder. It lets Prism recognize the same instance after a move and reject
a replacement that reuses its ID. Keep the marker when moving the folder or
backing it up; a full game-folder backup that includes hidden files keeps it.

Horizon manages its own game builds, Microsoft accounts, preparation and mods.
Use the Horizon launcher to install a build and sign in with an account that
owns Minecraft: Java Edition. The corresponding CLI commands are
`horizon-launcher install latest`, `horizon-launcher account sign-in`, and
`horizon-launcher prepare` (if the build needs assets prepared).
**Remove from Prism** removes only the link; the Horizon game folder and saves
remain. Manage Horizon mods, servers, and exports through the official Horizon
launcher, and select resource packs in the Horizon game. Prism's mod browser,
server editor, resource-pack editor, and export are for Java instances.

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
  export.rs       Prism zip, Modrinth mrpack and CurseForge modpack export
  news.rs         Atom/RSS news feed

crates/prism-ui (GPUI)
  main_window.rs      main window: toolbar, grouped instance grid, sidebar, console, news/status
  instance_window.rs  per-instance pages: Log, Other Logs, Version (loader install), Mods, Browse Mods
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
- Modrinth: browse and install mods, resource packs, and shader packs; import `.mrpack` modpacks
- Import a Modrinth modpack or release URL to create a managed instance. **Edit → Managed Pack**
  shows the installed version, available versions, and changelogs, and updates selected versions
  while Minecraft is closed. Pack-owned files and overrides are replaced; existing saves and
  untracked files are kept. Local archives without a recorded provider project remain unmanaged.
- CurseForge: import modpack zips
- Offline accounts; MSA device-code login implemented end-to-end
- **Accounts → Manage Skins** for Microsoft accounts: persistent local skin library,
  file/URL/username imports, rename and confirmed delete, Classic/Slim previews,
  skin uploads, and cape selection/hiding. Offline accounts cannot manage skins.
- Global + per-instance settings (memory, Java, window size). Java arguments inherit the global setting unless the instance overrides them.
- Launcher HTTP proxy settings (system, direct, HTTP and SOCKS5); the game manages its own network settings.
- API Keys page for CurseForge and Imgur. Environment variables `PRISM_RS_CURSEFORGE_API_KEY` and `PRISM_RS_IMGUR_CLIENT_ID` override saved values.
- Live console with log capture, search, copy, and confirmed mclo.gs sharing; kill running instances
- Other Logs page: browse and decompress Java logs, search, copy, share, and confirm deletion
- Screenshots page: thumbnail grid, rename, confirmed delete, and opt-in Imgur upload
- Quick Play singleplayer worlds and multiplayer servers, with dialogs and headless prepare options
- Worlds page: save details, rename, copy, confirmed delete, and folder browsing
- News feed, grouped instance list, persistent config
- Instance export: Prism zip and Modrinth mrpack (CLI and instance toolbar), CurseForge zip (instance toolbar)
- **Edit → Mods → Export Mod List**: preview, copy, or save HTML, Markdown, plain text, JSON,
  CSV, or a custom template. Choose whether to include version, authors, URL, and file name.
- Direct instance launch from the CLI and Linux desktop shortcuts
- Version page: change Minecraft, loader, and LWJGL versions; manage ordered jar mods and extra classpath JARs shared by GUI and CLI launch

- FTB: browse/search/sort the public catalog, select a version and install; import existing
  FTB App instances (worlds, JVM arguments, playtime and icon). Available from Add Instance
  or Import. Headless: `prism-rs import ftb:<pack-id>[:<version-id>]` (latest when omitted)
  or `prism-rs import <ftb-app-instance-directory>`. Restricted CurseForge files are listed
  in `curseforge-missing-files.txt`; automatic resolution uses the configured CurseForge key.
  FTB provenance is recorded, but FTB updates are not connected to the Modrinth-only updater.

Not yet at parity (tracked in the issue tracker of your heart): ATL/Technic pack
import, themes/translations, updater, generic component JSON editing and import,
custom client-JAR replacement, Java agents, profiling tools, and per-instance console window options.

## License

MIT (see LICENSE). The original PrismLauncher is GPL-3.0; this is a clean-room reimplementation in
a different language, following its feature set and file formats for interoperability.
