# Security model

Omazen deliberately installs privileged browser code. This is more powerful than a Zen Mod or WebExtension and must be treated like startup code.

## Trust boundary

fx-autoconfig allows JavaScript under a profile's `chrome` directory to execute with browser privileges. A malicious process that can modify those files can take over the browser context. Omazen does not make that mechanism safe; it minimizes and documents its use.

Installed privileged files per Zen profile:

```text
chrome/utils/boot.sys.mjs
chrome/utils/chrome.manifest
chrome/utils/fs.sys.mjs
chrome/utils/module_loader.mjs
chrome/utils/uc_api.sys.mjs
chrome/utils/utils.sys.mjs
chrome/JS/omazen-bridge.uc.js
chrome/JS/Omazen/OmazenParent.sys.mjs
chrome/JS/Omazen/OmazenChild.sys.mjs
chrome/JS/Omazen/OmazenPalette.sys.mjs
chrome/JS/Omazen/OmazenWatcher.sys.mjs
chrome/JS/Omazen/OmazenBoosts.sys.mjs
chrome/JS/Omazen/omazen-chrome-v1.7.0.css
chrome/JS/Omazen/omazen-content-v1.7.0.css
```

The same files are installed into the dedicated profile of each web app created
with `omazen webapp install --theme`. Web apps created without `--theme` receive
no privileged files at all, only a `userChrome.css` that squares the page's
corners.

Program-level files for the supported Zen package:

```text
/opt/zen-browser-bin/config.js
/opt/zen-browser-bin/defaults/pref/config-prefs.js
/opt/zen-browser-bin/defaults/pref/omazen-prefs.js
```

The first two may be reused from a compatible pre-existing fx-autoconfig installation. Omazen never silently replaces a foreign program config or partial profile runtime.

## Reductions

- No remote download, update or execution at runtime.
- The installed command is the Rust executable itself, with no shell launcher
  or alternate CLI implementation. Rust is a build dependency and the
  installed runtime never downloads a toolchain.
- Dependencies are pinned by commit and SHA-256.
- The visual CI job pins the Zen release version and verifies its SHA-256 before
  extraction or execution; the archive's version metadata remains a separate
  defense-in-depth check.
- No `eval`, dynamic import path, local port, native-messaging host or page-exposed API.
- The event watcher launches only the fixed `/usr/bin/inotifywait` executable
  with fixed arguments and the private Omazen state directory. It does not use
  a shell, listen on a local port or accept commands from page content.
- Palette and log paths are fixed; JSON cannot select a path. Logging is bounded
  to the active `bridge.log` plus one rotated `bridge.log.1` archive.
- JSON is size-limited and strictly validated before use.
- Only normalized colors and mode cross the actor boundary.
- The actor matches a fixed internal-page allowlist and the built-in DevTools chrome namespace, never ordinary web origins.
- Most CSS is static and shipped with Omazen. For isolated Passwords, Print and DevTools processes, the eight strictly validated hex colors and validated mode are inserted into fixed, URL-scoped internal-page rules. The same generated user sheet contains a separate fixed rule for `http:`, `https:` and `file:` documents that can set only `scrollbar-color`; it provides no script, DOM access or page-exposed API. JSON cannot supply selectors, property names or URLs in either scope.
- Logs contain timestamps, fixed event names, mode, accent, a stable opaque
  profile identifier and validation errors—not URLs, page titles, profile paths
  or browsing data.
- Logs rotate at 128 KiB.
- `omazen report` packages only sanitized text diagnostics, a bounded recent log
  fragment and SHA-256 metadata; it does not copy profiles, palette contents or
  other user files. Paths below the configured home directory are rendered as
  `$HOME`. Users should still inspect the archive before sharing it.
- Disable is live and uninstall is ownership/hash aware.
- Page theming through Zen boosts is opt-in per web app. The bridge starts the
  boost driver only when its own profile directory lies inside the Omazen web
  apps directory *and* carries the `omazen.webapp.hosts` preference that
  `omazen webapp install --theme` writes; every other profile, including all
  regular Zen profiles, returns before the boost manager is even loaded. The
  driver writes only numeric color parameters derived from the validated accent
  and mode, plus one fixed stylesheet that makes the page root transparent,
  into one boost per recorded host that it names and owns. Hosts must
  be plain DNS names, and boosts the user created are left alone. Disabling
  Omazen removes the owned boosts.
- A web app's URL never appears in a launcher's `Exec` line or in the Omarchy
  menu: launchers carry only the web app's identifier, and
  `omazen webapp launch` passes the URL to Zen as a single argument. URLs must
  be `http` or `https` with a plain host. `omazen webapp` runs its helpers
  (`curl`, `gum`, `hyprctl`, Omarchy's menu and notification commands) with
  fixed argument vectors, and a downloaded icon is kept only when its content
  is an image.
- `omazen setup` edits the user's Omarchy menu extension only between its own
  marker comments, keeps the file's permissions, writes through a symlinked
  file instead of replacing it, and never rewrites entries outside the block.
- External palette-provider mode can skip the Omarchy hook, but it does not
  bypass palette validation, fixed paths, loader integrity, or ownership
  checks. The external provider must supply a trusted local `colors.toml` path
  and invoke synchronization itself.

## Updates

Zen package upgrades can replace program-level files. Omazen does not fight the package manager or auto-repair as root. Run `omazen doctor`; if the owned loader is missing, run `omazen setup` interactively. `sudo` is used only when a terminal is available, otherwise the installer asks through `pkexec`.

Setup and uninstall validate ownership, hashes, sources and destinations before
elevation, then apply the complete program-file batch through one privileged
helper invocation. The helper accepts only the three documented paths below
`/opt/zen-browser-bin`; it is not a general privileged command runner.

## Uninstall limits

If another user script is found, Omazen leaves an owned shared fx-autoconfig program loader in place rather than breaking that script. If an owned file has been modified, it is also retained. In both cases ownership records remain so the user can inspect and resolve the shared state explicitly.
