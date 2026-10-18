# Installer and Explorer integration

Inkwell ships as a per-machine Windows Installer package (MSI), built by Tauri's WiX bundler from our own template
(`src-tauri/wix/main.wxs`). **The MSI is unsigned**: Windows SmartScreen and the UAC prompt show "Unknown publisher"
until a code-signing certificate is added (Tauri's `bundle.windows.certificateThumbprint`, no other change).

## Build

```
$env:LIBCLANG_PATH = '<LLVM>\bin'   # LLVM's bin folder (libclang)
pnpm tauri build
```

Run it from the repo root. Output: `src-tauri/target/release/bundle/msi/Inkwell_<version>_x64_en-US.msi`.

Before a release, run `node third_party/dependency-licenses.mjs` to regenerate `third_party/RUST-AND-NPM-DEPENDENCIES.txt`.

- `bundle.targets` is `["msi"]`; NSIS is left out (it adds nothing the MSI lacks, and an MSI is what IT tooling expects).
- WiX 3.14 comes from `%LOCALAPPDATA%\tauri\WixTools314` (the CLI downloads it on first use).
- Icons are drawn by `scripts/make-icons.ps1` from the ink-drop mark in the title bar (`Chrome.tsx`, accent
  `#2e4b8f`): the app icon (`icon.ico`, a white drop on an accent tile, plus the PNG set) and the PDF document icon
  (`pdf-document.ico`, a page with the drop on it). Rerun the script after changing the mark. `icon.icns` (for
  tauri on macOS) comes from the same mark: `pwsh scripts/make-icons.ps1 -Master <1024.png>`, then
  `pnpm tauri icon <1024.png> -o <scratch dir>` and copy only `icon.icns` out of it.

## What the MSI does

| Item | Choice |
|---|---|
| Product | Inkwell, publisher "Berkay", `UpgradeCode` `A7BB0957-BACE-46F2-A766-0F5C17191026` (never change it) |
| Scope | Per machine, `C:\Program Files\Inkwell`, one UAC prompt |
| Upgrade | A newer version removes the old one and installs in the same folder (MajorUpgrade, same-version reinstall allowed). An older MSI refuses with a message. User data and the desktop-shortcut choice survive. |
| Start menu | `Inkwell` shortcut directly in Programs (no folder) |
| Desktop shortcut | Off by default: `msiexec /i Inkwell.msi DESKTOPSHORTCUT=1`. An upgrade keeps the choice. |
| Licences | `licenses\` under the install folder (LICENSE-AGPL-3.0.txt, THIRD-PARTY-NOTICES, RUST-AND-NPM-DEPENDENCIES, `mupdf\<lib>` licences, the fonts' OFL texts, pdfcraft MIT/Apache/NOTICE); the installer shows the AGPL as its licence page |
| WebView2 | `embedBootstrapper`, silent: the 1.7 MB evergreen bootstrapper is inside the MSI and runs only when no WebView2 runtime is installed (never on Windows 11, which ships it). It needs the network at that moment; the rest of the install does not. The 130 MB offline installer is not worth it for an app that already needs Windows 11's runtime. |
| Silent install | `msiexec /i Inkwell.msi /qn` (add `DESKTOPSHORTCUT=1`, `INSTALLDIR="D:\Apps\Inkwell"`) |
| Uninstall | Settings > Apps, or `msiexec /x Inkwell.msi` |

### Registry (all under `HKLM`, 64-bit view)

| Key | Purpose |
|---|---|
| `Software\Inkwell` (`InstallDir`, shortcut flags) | where the app lives; read by the next version's installer |
| `Software\Inkwell\Capabilities` (+ `FileAssociations` `.pdf` = `Inkwell.PDF`) | Settings > Default apps lists Inkwell as able to open PDFs |
| `Software\RegisteredApplications` value `Inkwell` | points Windows at the capability above |
| `Software\Classes\Inkwell.PDF` (`DefaultIcon` = `pdf-document.ico`, `shell\open\command`) | what a PDF is when Inkwell opens it |
| `Software\Classes\.pdf\OpenWithProgids` value `Inkwell.PDF` | Inkwell appears under "Open with" |
| `Software\Classes\Applications\inkwell.exe` | the classic "Open with > Choose another app" entry |
| `Software\Classes\SystemFileAssociations\.pdf\shell\Inkwell.Open` | context-menu verb "Open with Inkwell" |

**Never the default.** The installer writes nothing to `.pdf`'s default value, to `HKCU\...\FileExts\.pdf\UserChoice`
or to any `Extension`/`ProgId` advertisement. Windows asks the user in Settings > Apps > Default apps (or the
"How do you want to open this file?" prompt). On Windows 11 the context-menu verb is under "Show more options" (the
classic menu); "Open with" in the new menu lists Inkwell too.

### Uninstall

Removes the files, both shortcuts and every key above (the value under `.pdf\OpenWithProgids` and
`RegisteredApplications` is removed, the shared keys stay). **The user data folder stays**:
`%APPDATA%\app.inkwell.desktop` (recent files, stamps, history store, sign data, tool settings) and the WebView2
profile in `%LOCALAPPDATA%\app.inkwell.desktop`. Delete them by hand for a clean slate.

## Opening files

- `inkwell.exe <file.pdf> ...` (Explorer's double click, "Open with", the verb) opens the files in tabs. Relative paths
  resolve against the caller's working directory; flags (`-x`) are skipped.
- **Single instance** (`tauri-plugin-single-instance`): a second launch sends its arguments and working directory to
  the running Inkwell, which un-minimises and focuses its window and opens the PDFs the way a drop does (open tokens
  from `paths.rs`, event `files-dropped`). The second process exits. Explorer starts one process per selected file;
  they all end up in the one window. Only existing `.pdf` files count; an argument is a file when it names one, so
  `-x` is skipped and `-x.pdf` is opened.
- **Startup order.** Files forwarded before the window has loaded wait in `launch::Forwarded` (capped at 1000). The UI
  registers its `files-dropped` listener first and only then calls `initial_files`, which hands over the waiting files
  and switches later ones to being emitted; so none can fall in between.
- Debug builds and runs with a scratch `INKWELL_DATA_DIR` (tests, benchmarks, screenshot scripts) stay independent of an
  installed Inkwell and of each other; set `INKWELL_SINGLE_INSTANCE=1` to test forwarding with one.
- The verb has `MultiSelectModel=Player`, so "Open with Inkwell" stays on the menu for any number of selected files
  (Explorer hides a default-model verb above 15).
- Code: `src-tauri/src/launch.rs` (argument parsing, the wait queue, unit tests), `lib.rs` (`initial_files`,
  `on_second_launch`, `deliver_pdfs`), `App.tsx` (listener, then `initialFiles`).

Not done: `SHChangeNotify(SHCNE_ASSOCCHANGED)` after install and uninstall. WiX 3 has no built-in for it (it would need a
custom action DLL or a script), and nothing here depends on it: Explorer reads the verb and the Open with list when it
builds a menu, and the document icon only shows once Inkwell is the user's default, which is their choice in Settings
(Windows broadcasts the change itself then).

Skipped: a jump list / recent entries on the taskbar. Windows keeps its own "recent" for files opened through the
shell; an app-side list would need `SHAddToRecentDocs` and a decision about what the app's own recent list is.

## Checking an installer

`scripts/installer-check.ps1 -Msi <msi> -Pdf '<copy1.pdf>;<copy2.pdf>'` (elevated shell) installs, checks the list above,
launches the verb for each file and checks for one running instance with every file in its recent list, uninstalls,
and checks nothing is left. `-Burst` starts all files at once (a multi-select; try it with 20 copies). It never touches
the default PDF app and uses a scratch data folder.

The shortcut components use `HKCU\Software\Inkwell` marker keys as key paths (Windows Installer requires a
per-user key path for per-user shortcuts); uninstall removes them.

What has been verified so far: unit tests for the argument parsing and the wait queue, the MSI tables, and a full
install / Open with / forward / uninstall run of a per-user variant of the same template (HKCU roots, different
`UpgradeCode`). The per-machine MSI itself needs the elevated run above, which is a one-time manual step (UAC).
