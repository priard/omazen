// SPDX-License-Identifier: GPL-3.0-only
// See NOTICE for the required Omazen project attribution terms.

//! Zen web apps, kept alongside Omarchy's Chromium-based `omarchy webapp`.
//!
//! Each web app opens one site in its own isolated Zen profile, started in
//! compact mode (no sidebar, no toolbar) under a unique Wayland class, so the
//! launcher, alt-tab and Hyprland rules treat it as a separate application.
//! `--theme` additionally installs Omazen's runtime into that one profile and
//! records the hosts it serves; the bridge then tints those pages with the
//! active Omarchy theme through a Zen boost. Regular Zen profiles never carry
//! that preference, so ordinary browsing is not recolored.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    DoctorReport, RuntimePaths, forget_owned_file, install_fx_profile_runtime,
    install_profile_files, install_user_bytes, json_escape, manifest_entries,
    program_has_compatible_fx, remove_owned_file, runtime_paths, sha256_file,
};

const LAUNCHER_PREFIX: &str = "omazen-webapp-";
const LAUNCHER_MARKER: &str = "X-Omazen-Webapp";
const CLASS_PREFIX: &str = "omazen-webapp-";
const FALLBACK_ICON: &str = "zen-browser";
const WEB_GLYPH: &str = "\u{f059f}";
const INSTALL_LAUNCHER: &str = "org.omazen.WebAppInstall.desktop";
const REMOVE_LAUNCHER: &str = "org.omazen.WebAppRemove.desktop";
const MENU_BEGIN: &str = "// >>> omazen web apps";
const MENU_END: &str = "// <<< omazen web apps";
const MAX_PAGE_BYTES: usize = 100_000;

const USAGE: &str = concat!(
    "Usage: omazen webapp <command> [arguments]\n",
    "\n",
    "Commands:\n",
    "  install [--theme [--invert]] [name url [icon]]\n",
    "                    Create a web app with its own Zen profile; without a\n",
    "                    name and URL, ask for them interactively\n",
    "                    --theme   tint its pages with the active Omarchy theme\n",
    "                    --invert  the site is light: invert it in dark themes\n",
    "  remove [name]     Remove a web app, its launcher and its profile\n",
    "  list              List installed web apps\n",
    "  launch <id>       Start or focus a web app (used by its launcher)\n",
    "  help              Show this help\n"
);

const PROFILE_PREFS: &str = r#"// Written by `omazen webapp install`. A web app profile skips first-run,
// update and default-browser prompts, and starts in compact mode with the tab
// bar and the toolbar hidden and no hover reveal, so only the page is visible.
user_pref("zen.welcome-screen.seen", true);
user_pref("zen.updates.show-update-notification", false);
user_pref("zen.view.compact.enable-at-startup", true);
user_pref("zen.view.compact.hide-tabbar", true);
user_pref("zen.view.compact.hide-toolbar", true);
user_pref("zen.view.compact.show-sidebar-and-toolbar-on-hover", false);
// Zen frames the page with a gap and a rounded, shadowed card, which in compact
// mode also reads as a panel edge along the left side. A web app has the window
// border as its only frame.
user_pref("zen.theme.content-element-separation", 0);
user_pref("zen.theme.border-radius", 0);
// A page in another language would otherwise open the translations panel,
// which in compact mode slides the toolbar out over the page.
user_pref("browser.translations.automaticallyPopup", false);
// Lets the managed chrome/userChrome.css square the page's corners.
user_pref("toolkit.legacyUserProfileCustomizations.stylesheets", true);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
"#;

#[derive(Debug)]
struct WebApp {
    slug: String,
    name: String,
    url: String,
    icon: String,
    dir: PathBuf,
    hosts: Option<String>,
    invert: bool,
}

impl WebApp {
    fn profile(&self) -> PathBuf {
        self.dir.join("profile")
    }

    fn class(&self) -> String {
        format!("{CLASS_PREFIX}{}", self.slug)
    }

    fn themed(&self) -> bool {
        self.hosts.is_some()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct InstallRequest {
    name: String,
    url: String,
    icon: Option<String>,
    theme: bool,
    invert: bool,
}

pub(crate) fn run(arguments: &[OsString]) -> Result<(), String> {
    let result = dispatch(arguments);
    // Launched from the Omarchy menu or the app launcher there is no terminal,
    // so a failure must also arrive as a desktop notification.
    if let Err(message) = &result
        && !message.is_empty()
    {
        notify("Zen web app", message, "normal");
    }
    result
}

fn dispatch(arguments: &[OsString]) -> Result<(), String> {
    let (command, rest): (Option<&str>, &[OsString]) = match arguments.split_first() {
        Some((command, rest)) => (command.to_str(), rest),
        None => (Some("help"), &[]),
    };
    match command {
        Some("install") => install(rest),
        Some("remove") => remove(rest),
        Some("list") => {
            if !rest.is_empty() {
                return Err("webapp list takes no arguments".to_owned());
            }
            list()
        }
        Some("launch") => launch(rest),
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        other => {
            eprint!("{USAGE}");
            Err(format!("unknown webapp command: {}", other.unwrap_or("?")))
        }
    }
}

fn testing() -> bool {
    env::var_os("OMAZEN_TESTING").as_deref() == Some(OsStr::new("1"))
}

fn utf8(argument: &OsStr) -> Result<&str, String> {
    argument
        .to_str()
        .ok_or_else(|| "web app arguments must be valid UTF-8".to_owned())
}

// ---------------------------------------------------------------------------
// install

fn parse_install_arguments(arguments: &[OsString]) -> Result<InstallRequest, String> {
    let mut request = InstallRequest::default();
    let mut positional = Vec::new();
    let mut options_done = false;
    for argument in arguments {
        let argument = utf8(argument)?;
        match argument {
            "--theme" if !options_done => request.theme = true,
            "--invert" if !options_done => {
                request.theme = true;
                request.invert = true;
            }
            "--" if !options_done => options_done = true,
            flag if !options_done && flag.starts_with("--") => {
                return Err(format!("unknown webapp install option: {flag}"));
            }
            value => positional.push(value.to_owned()),
        }
    }
    match positional.as_slice() {
        [] => {}
        [name, url] => {
            request.name.clone_from(name);
            request.url.clone_from(url);
        }
        [name, url, icon] => {
            request.name.clone_from(name);
            request.url.clone_from(url);
            request.icon = Some(icon.clone());
        }
        _ => {
            return Err(
                "usage: omazen webapp install [--theme [--invert]] [name url [icon]]".to_owned(),
            );
        }
    }
    Ok(request)
}

fn install(arguments: &[OsString]) -> Result<(), String> {
    let paths = runtime_paths()?;
    let mut request = parse_install_arguments(arguments)?;
    if request.name.is_empty() {
        prompt_install(&mut request)?;
    }
    let name = validate_name(&request.name)?;
    let url = normalize_url(&request.url)?;
    let slug = slugify(&name);
    if !is_valid_slug(&slug) {
        return Err(format!("web app name needs a letter or digit: {name}"));
    }
    let dir = paths.webapps_dir.join(&slug);
    if dir.exists() || launcher_path(&paths, &slug).exists() {
        return Err(format!(
            "a web app named '{name}' already exists; remove it first"
        ));
    }
    fs::create_dir_all(dir.join("profile")).map_err(|error| error.to_string())?;
    match create_webapp(&paths, &request, &name, &url, &dir) {
        Ok(app) => {
            let note = match (app.themed(), app.invert) {
                (false, _) => "",
                (true, false) => " (Omarchy theme)",
                (true, true) => " (Omarchy theme, inverted in dark themes)",
            };
            println!("Installed Zen web app '{}' → {}{note}", app.name, app.url);
            println!("Find it in the app launcher (SUPER + SPACE).");
            notify("Zen web app installed", &app.name, "low");
            Ok(())
        }
        Err(error) => {
            discard_webapp(&paths, &slug, &dir);
            Err(error)
        }
    }
}

fn prompt_install(request: &mut InstallRequest) -> Result<(), String> {
    if !io::stdin().is_terminal() {
        return Err(
            "webapp install needs a name and URL, or a terminal to ask for them".to_owned(),
        );
    }
    println!("\x1b[32mCreate a Zen web app you can start from the app launcher.\n\x1b[0m");
    request.name = gum_input("Name> ", "My favorite web app")?;
    request.url = gum_input("URL> ", "https://example.com")?;
    if !request.theme
        && gum_confirm(
            "Match the Omarchy theme? (tints the page, follows theme switches)",
            true,
        )
    {
        request.theme = true;
        request.invert = gum_confirm(
            "Is this a light site? (inverts it while the theme is dark)",
            false,
        );
    }
    Ok(())
}

fn gum_input(prompt: &str, placeholder: &str) -> Result<String, String> {
    let output = Command::new("gum")
        .args(["input", "--prompt", prompt, "--placeholder", placeholder])
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("gum is required to ask for web app details: {error}"))?;
    if !output.status.success() {
        // Cancelled: leave quietly, like Omarchy's own web app installer.
        return Err(String::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn gum_confirm(prompt: &str, default: bool) -> bool {
    Command::new("gum")
        .args(["confirm", &format!("--default={default}"), prompt])
        .status()
        .is_ok_and(|status| status.success())
}

fn create_webapp(
    paths: &RuntimePaths,
    request: &InstallRequest,
    name: &str,
    url: &str,
    dir: &Path,
) -> Result<WebApp, String> {
    let slug = slugify(name);
    let icon = install_icon(paths, &slug, request.icon.as_deref(), url)?;
    write_text(&dir.join("name"), name)?;
    write_text(&dir.join("url"), url)?;
    write_text(&dir.join("icon"), &icon)?;
    let profile = fs::canonicalize(dir.join("profile")).map_err(|error| error.to_string())?;
    let hosts = if request.theme {
        let hosts = resolve_hosts(url);
        if hosts.is_empty() {
            return Err(format!("could not determine the host of {url}"));
        }
        let hosts = hosts.join(",");
        write_text(&dir.join("hosts"), &hosts)?;
        if request.invert {
            write_text(&dir.join("invert"), "1")?;
        }
        Some(hosts)
    } else {
        None
    };
    write_profile_prefs(&profile, hosts.as_deref(), request.invert)?;
    write_user_chrome(&profile)?;
    if request.theme {
        enable_theme(paths, &profile)?;
    }
    let app = load_webapp(dir).ok_or_else(|| format!("could not read back web app '{name}'"))?;
    write_launcher(paths, &app)?;
    refresh_desktop_database(paths);
    Ok(app)
}

fn theme_prefs(hosts: &str, invert: bool) -> String {
    format!(
        concat!(
            "// Written by `omazen webapp install --theme`. Omazen's bridge reads these\n",
            "// to tint this web app's pages with the active Omarchy theme.\n",
            "user_pref(\"omazen.webapp.hosts\", \"{hosts}\");\n",
            "user_pref(\"omazen.webapp.invert\", {invert});\n",
            "// Glass: Zen draws the window with an alpha channel and lets the page be\n",
            "// transparent, so the compositor's blur shows through Omazen's tint.\n",
            "user_pref(\"zen.widget.linux.transparency\", true);\n",
            "user_pref(\"browser.tabs.allow_transparent_browser\", true);\n",
            "// fx-autoconfig, which Omazen's runtime needs, otherwise announces itself\n",
            "// in a notification bar on the web app's first start.\n",
            "user_pref(\"userChromeJS.firstRunShown\", true);\n"
        ),
        hosts = hosts,
        invert = invert
    )
}

fn managed_prefs(hosts: Option<&str>, invert: bool) -> String {
    let mut prefs = PROFILE_PREFS.to_owned();
    if let Some(hosts) = hosts {
        prefs.push_str(&theme_prefs(hosts, invert));
    }
    prefs
}

fn pref_name(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("user_pref(\"")?
        .split('"')
        .next()
}

// The preferences Omazen writes are regenerated; any preference the user
// added to the web app's user.js is kept after them.
fn merge_profile_prefs(managed: &str, existing: &str) -> String {
    let names: Vec<&str> = managed.lines().filter_map(pref_name).collect();
    let kept: Vec<&str> = existing
        .lines()
        .filter(|line| {
            pref_name(line)
                .is_some_and(|name| !names.contains(&name) && !name.starts_with("omazen.webapp."))
        })
        .collect();
    let mut text = managed.trim_end().to_owned();
    text.push('\n');
    if !kept.is_empty() {
        text.push_str("// Kept from this web app's own configuration.\n");
        for line in kept {
            text.push_str(line.trim());
            text.push('\n');
        }
    }
    text
}

/// Returns true when the file changed; Zen reads it on the web app's next start.
fn write_profile_prefs(profile: &Path, hosts: Option<&str>, invert: bool) -> Result<bool, String> {
    let path = profile.join("user.js");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let text = merge_profile_prefs(&managed_prefs(hosts, invert), &existing);
    if text == existing {
        return Ok(false);
    }
    fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(true)
}

const USER_CHROME_MARKER: &str = "omazen:webapp-managed";
const USER_CHROME: &str = r#"/* omazen:webapp-managed. Written by `omazen webapp install` and refreshed by
 * `omazen setup`; delete this marker to keep your own version of this file.
 *
 * Zen keeps a minimum radius on the page and Omazen rounds and shadows it; a
 * web app window has square corners like any other window. */
:root {
  --zen-webview-border-radius: 0px !important;
  --omazen-content-radius: 0px !important;
  --omazen-content-shadow: none !important;
}
"#;

/// Returns true when the file changed. A userChrome.css without the marker
/// belongs to the user and is left alone.
fn write_user_chrome(profile: &Path) -> Result<bool, String> {
    let path = profile.join("chrome/userChrome.css");
    match fs::read_to_string(&path) {
        Ok(existing) if existing == USER_CHROME || !existing.contains(USER_CHROME_MARKER) => {
            return Ok(false);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    fs::create_dir_all(profile.join("chrome")).map_err(|error| error.to_string())?;
    fs::write(&path, USER_CHROME).map_err(|error| error.to_string())?;
    Ok(true)
}

// A boost is keyed on the exact host of the page, so record both the host a
// web app opens on and the one it lands on after redirects.
fn resolve_hosts(url: &str) -> Vec<String> {
    let mut hosts: Vec<String> = url_host(url).into_iter().collect();
    if testing() {
        return hosts;
    }
    let landed = Command::new("curl")
        .args([
            "-sL",
            "-o",
            "/dev/null",
            "--max-time",
            "10",
            "-w",
            "%{url_effective}",
            url,
        ])
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|output| url_host(&String::from_utf8_lossy(&output.stdout)));
    if let Some(host) = landed
        && !hosts.contains(&host)
    {
        hosts.push(host);
    }
    hosts
}

fn enable_theme(paths: &RuntimePaths, profile: &Path) -> Result<(), String> {
    if !program_has_compatible_fx(&paths.zen_program_dir) {
        return Err("--theme needs Omazen's Zen loader; run `omazen setup` first".to_owned());
    }
    install_fx_profile_runtime(paths, profile)?;
    install_profile_files(paths, profile)
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, format!("{text}\n")).map_err(|error| error.to_string())
}

fn discard_webapp(paths: &RuntimePaths, slug: &str, dir: &Path) {
    if let Ok(profile) = fs::canonicalize(dir.join("profile")) {
        let _ = forget_profile_files(paths, &profile);
    }
    if is_valid_slug(slug) && dir.starts_with(&paths.webapps_dir) && dir != paths.webapps_dir {
        let _ = fs::remove_dir_all(dir);
    }
    let _ = remove_launcher(paths, slug);
    let _ = fs::remove_file(icon_path(paths, slug));
}

fn forget_profile_files(paths: &RuntimePaths, profile: &Path) -> Result<(), String> {
    for (path, _) in manifest_entries(&paths.profile_manifest) {
        if path.starts_with(profile) {
            forget_owned_file(&paths.profile_manifest, &path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// remove, list, launch

fn remove(arguments: &[OsString]) -> Result<(), String> {
    let paths = runtime_paths()?;
    let apps = installed_webapps(&paths);
    let name = if arguments.is_empty() {
        if apps.is_empty() {
            return Err("no Zen web apps are installed".to_owned());
        }
        match pick_webapp(&apps)? {
            Some(name) => name,
            None => return Ok(()),
        }
    } else {
        arguments
            .iter()
            .map(|argument| utf8(argument).map(str::to_owned))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ")
    };
    let slug = slugify(&name);
    let app = apps
        .into_iter()
        .find(|app| app.slug == slug)
        .ok_or_else(|| format!("'{name}' is not a Zen web app"))?;
    if running_address(&app.class()).is_some() {
        return Err(format!("'{}' is running; close it first", app.name));
    }
    remove_webapp_files(&paths, &app)?;
    println!("Removed Zen web app '{}'", app.name);
    notify("Zen web app removed", &app.name, "low");
    Ok(())
}

fn pick_webapp(apps: &[WebApp]) -> Result<Option<String>, String> {
    let output = Command::new("omarchy-menu-select")
        .arg("Remove Zen web app")
        .args(apps.iter().map(|app| app.name.as_str()))
        .args(["--", "--width", "520", "--maxheight", "520"])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("omarchy-menu-select is required to pick a web app: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let choice = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!choice.is_empty()).then_some(choice))
}

fn remove_webapp_files(paths: &RuntimePaths, app: &WebApp) -> Result<(), String> {
    if let Ok(profile) = fs::canonicalize(app.profile()) {
        forget_profile_files(paths, &profile)?;
    }
    remove_launcher(paths, &app.slug)?;
    let _ = fs::remove_file(icon_path(paths, &app.slug));
    if is_valid_slug(&app.slug)
        && app.dir.starts_with(&paths.webapps_dir)
        && app.dir != paths.webapps_dir
    {
        fs::remove_dir_all(&app.dir).map_err(|error| error.to_string())?;
    }
    refresh_desktop_database(paths);
    Ok(())
}

fn list() -> Result<(), String> {
    let paths = runtime_paths()?;
    for app in installed_webapps(&paths) {
        let theme = match (app.themed(), app.invert) {
            (false, _) => "",
            (true, false) => "theme",
            (true, true) => "theme,invert",
        };
        println!("{:<24} {:<13} {}", app.name, theme, app.url);
    }
    Ok(())
}

fn launch(arguments: &[OsString]) -> Result<(), String> {
    let [slug] = arguments else {
        return Err("usage: omazen webapp launch <id>".to_owned());
    };
    let slug = utf8(slug)?;
    if !is_valid_slug(slug) {
        return Err(format!("invalid web app id: {slug}"));
    }
    let paths = runtime_paths()?;
    let app = load_webapp(&paths.webapps_dir.join(slug))
        .ok_or_else(|| format!("unknown web app: {slug}"))?;
    if let Some(address) = running_address(&app.class()) {
        focus_window(&address);
        return Ok(());
    }
    let zen = paths.zen_program_dir.join("zen-bin");
    let mut command = Command::new("setsid");
    if command_exists("uwsm-app") {
        command.args(["uwsm-app", "--"]);
    }
    command
        .arg(&zen)
        .arg("--profile")
        .arg(app.profile())
        .arg("--name")
        .arg(app.class())
        .arg(&app.url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start Zen for '{}': {error}", app.name))?;
    Ok(())
}

fn running_address(class: &str) -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["clients", "-j"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    window_address(&String::from_utf8_lossy(&output.stdout), class)
}

fn focus_window(address: &str) {
    let focus = format!("hl.dsp.focus({{ window = \"address:{address}\" }})");
    let focused = Command::new("hyprctl")
        .args(["dispatch", &focus])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !focused {
        let _ = Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("address:{address}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn command_exists(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|directory| {
            fs::metadata(directory.join(name))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    })
}

fn notify(headline: &str, body: &str, urgency: &str) {
    if io::stderr().is_terminal() || testing() {
        return;
    }
    let _ = Command::new("omarchy-notification-send")
        .args(["-g", WEB_GLYPH, "-u", urgency, headline, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// ---------------------------------------------------------------------------
// installed web apps and their launchers

fn load_webapp(dir: &Path) -> Option<WebApp> {
    let slug = dir.file_name()?.to_str()?.to_owned();
    if !is_valid_slug(&slug) {
        return None;
    }
    let read = |leaf: &str| {
        fs::read_to_string(dir.join(leaf))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    Some(WebApp {
        name: read("name")?,
        url: read("url")?,
        icon: read("icon").unwrap_or_else(|| FALLBACK_ICON.to_owned()),
        hosts: read("hosts"),
        invert: dir.join("invert").is_file(),
        dir: dir.to_path_buf(),
        slug,
    })
}

fn installed_webapps(paths: &RuntimePaths) -> Vec<WebApp> {
    let Ok(entries) = fs::read_dir(&paths.webapps_dir) else {
        return Vec::new();
    };
    let mut apps: Vec<WebApp> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| load_webapp(&entry.path()))
        .collect();
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

/// Profiles of web apps installed with `--theme`; setup, doctor and
/// uninstall treat them like any other Omazen profile.
pub(crate) fn themed_profiles(paths: &RuntimePaths) -> Vec<PathBuf> {
    installed_webapps(paths)
        .into_iter()
        .filter(WebApp::themed)
        .filter_map(|app| fs::canonicalize(app.profile()).ok())
        .collect()
}

fn launcher_path(paths: &RuntimePaths, slug: &str) -> PathBuf {
    paths
        .applications_dir
        .join(format!("{LAUNCHER_PREFIX}{slug}.desktop"))
}

fn icon_path(paths: &RuntimePaths, slug: &str) -> PathBuf {
    paths.icons_dir.join(format!("{LAUNCHER_PREFIX}{slug}.png"))
}

fn is_our_launcher(path: &Path, slug: &str) -> bool {
    let marker = format!("{LAUNCHER_MARKER}={slug}");
    fs::read_to_string(path).is_ok_and(|text| text.lines().any(|line| line == marker))
}

fn omazen_command(paths: &RuntimePaths) -> PathBuf {
    paths.local_bin_dir.join("omazen")
}

fn launcher_contents(paths: &RuntimePaths, app: &WebApp) -> String {
    let exec = format!(
        "{} webapp launch {}",
        exec_quote(&omazen_command(paths).to_string_lossy()),
        app.slug
    );
    format!(
        concat!(
            "[Desktop Entry]\n",
            "Version=1.0\n",
            "Type=Application\n",
            "Name={name}\n",
            "Comment={name} (Zen web app)\n",
            "Exec={exec}\n",
            "Icon={icon}\n",
            "Terminal=false\n",
            "StartupNotify=true\n",
            "StartupWMClass={class}\n",
            "{marker}={slug}\n"
        ),
        name = desktop_escape(&app.name),
        exec = desktop_escape(&exec),
        icon = desktop_escape(&app.icon),
        class = app.class(),
        marker = LAUNCHER_MARKER,
        slug = app.slug,
    )
}

fn write_launcher(paths: &RuntimePaths, app: &WebApp) -> Result<(), String> {
    let path = launcher_path(paths, &app.slug);
    if path.exists() && !is_our_launcher(&path, &app.slug) {
        return Err(format!(
            "refusing to replace a launcher Omazen did not create: {}",
            path.display()
        ));
    }
    fs::create_dir_all(&paths.applications_dir).map_err(|error| error.to_string())?;
    fs::write(&path, launcher_contents(paths, app)).map_err(|error| error.to_string())?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())
}

fn remove_launcher(paths: &RuntimePaths, slug: &str) -> Result<(), String> {
    let path = launcher_path(paths, slug);
    if is_our_launcher(&path, slug) {
        fs::remove_file(&path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn refresh_desktop_database(paths: &RuntimePaths) {
    let _ = Command::new("update-desktop-database")
        .arg(&paths.applications_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// ---------------------------------------------------------------------------
// icons

fn install_icon(
    paths: &RuntimePaths,
    slug: &str,
    reference: Option<&str>,
    url: &str,
) -> Result<String, String> {
    let name = format!("{LAUNCHER_PREFIX}{slug}");
    let destination = icon_path(paths, slug);
    match reference.map(str::trim).filter(|value| !value.is_empty()) {
        None => {
            if !testing() && fetch_site_icon(url, &destination) {
                refresh_icon_cache(paths);
                return Ok(name);
            }
            eprintln!("No icon found for {url}; using Zen's icon (pass one as the third argument)");
            Ok(FALLBACK_ICON.to_owned())
        }
        Some(value) if is_http_url(value) => {
            if download_icon(value, &destination) {
                refresh_icon_cache(paths);
                Ok(name)
            } else {
                Err(format!("could not download an image from {value}"))
            }
        }
        Some(value) if Path::new(value).is_file() => {
            let bytes = fs::read(value).map_err(|error| error.to_string())?;
            if !looks_like_image(&bytes) {
                return Err(format!("not an image file: {value}"));
            }
            fs::create_dir_all(&paths.icons_dir).map_err(|error| error.to_string())?;
            fs::write(&destination, bytes).map_err(|error| error.to_string())?;
            refresh_icon_cache(paths);
            Ok(name)
        }
        Some(value) => themed_icon_name(value),
    }
}

// Omarchy accepts both "HEY" and the historical "HEY.png" for bundled icons.
fn themed_icon_name(value: &str) -> Result<String, String> {
    let name = [".png", ".svg"]
        .iter()
        .find_map(|extension| value.strip_suffix(extension))
        .unwrap_or(value);
    if !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character))
    {
        Ok(name.to_owned())
    } else {
        Err(format!(
            "icon must be a URL, an image file or an icon name: {value}"
        ))
    }
}

fn refresh_icon_cache(paths: &RuntimePaths) {
    if let Some(hicolor) = paths.icons_dir.parent().and_then(Path::parent) {
        let _ = Command::new("gtk-update-icon-cache")
            .arg(hicolor)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// Prefer the site's own high-resolution icon, then the well-known path, then
// Google's favicon service, which answers sites without one with a 404.
fn fetch_site_icon(url: &str, destination: &Path) -> bool {
    let Some(origin) = url_origin(url) else {
        return false;
    };
    let page = Command::new("curl")
        .args(["-fsSL", "--max-time", "5", url])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let end = output.stdout.len().min(MAX_PAGE_BYTES);
            String::from_utf8_lossy(&output.stdout[..end]).into_owned()
        })
        .unwrap_or_default();
    let declared = apple_touch_icon_href(&page).map(|href| resolve_href(&origin, &href));
    declared.is_some_and(|icon| download_icon(&icon, destination))
        || download_icon(&format!("{origin}/apple-touch-icon.png"), destination)
        || download_icon(
            &format!("https://www.google.com/s2/favicons?domain={url}&sz=256"),
            destination,
        )
}

fn download_icon(url: &str, destination: &Path) -> bool {
    let Some(directory) = destination.parent() else {
        return false;
    };
    if fs::create_dir_all(directory).is_err() {
        return false;
    }
    let temporary = directory.join(format!(".omazen-icon.{}", process::id()));
    let downloaded = Command::new("curl")
        .args(["-fsSL", "--max-time", "10", "-o"])
        .arg(&temporary)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
        && fs::read(&temporary).is_ok_and(|bytes| looks_like_image(&bytes));
    if downloaded && fs::rename(&temporary, destination).is_ok() {
        return true;
    }
    let _ = fs::remove_file(&temporary);
    false
}

fn looks_like_image(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"GIF8")
        || bytes.starts_with(&[0, 0, 1, 0])
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
    {
        return true;
    }
    let head = &bytes[..bytes.len().min(512)];
    String::from_utf8_lossy(head)
        .to_ascii_lowercase()
        .contains("<svg")
}

fn apple_touch_icon_href(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<link") {
        let start = cursor + offset;
        let end = lower[start..]
            .find('>')
            .map_or(lower.len(), |position| start + position);
        let tag = &lower[start..end];
        if tag.contains("apple-touch-icon")
            && let Some(position) = tag.find("href=")
        {
            let value = &html[start + position + 5..end];
            if let Some(quote) = value
                .chars()
                .next()
                .filter(|quote| *quote == '"' || *quote == '\'')
                && let Some(close) = value[1..].find(quote)
            {
                let href = value[1..=close].trim();
                if !href.is_empty() {
                    return Some(href.to_owned());
                }
            }
        }
        cursor = end;
    }
    None
}

fn resolve_href(origin: &str, href: &str) -> String {
    if is_http_url(href) {
        href.to_owned()
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if href.starts_with('/') {
        format!("{origin}{href}")
    } else {
        format!("{origin}/{href}")
    }
}

// ---------------------------------------------------------------------------
// names, URLs and desktop-entry syntax

fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a web app name is required".to_owned());
    }
    // The name becomes a directory and a launcher; refuse rather than rename
    // what the user typed (most often a URL entered in the name field).
    if name.contains('/') {
        return Err(format!("web app name cannot contain '/': {name}"));
    }
    if name.chars().any(char::is_control) || name.chars().count() > 100 {
        return Err("web app name must be plain text of at most 100 characters".to_owned());
    }
    Ok(name.to_owned())
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.extend(character.to_lowercase());
        } else {
            separator = true;
        }
    }
    slug
}

fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && slug
            .chars()
            .all(|character| character == '-' || character.is_alphanumeric())
        && slug.chars().all(|character| !character.is_uppercase())
}

fn is_http_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn normalize_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("a web app URL is required".to_owned());
    }
    if trimmed
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("web app URL must not contain whitespace".to_owned());
    }
    let url = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    if !is_http_url(&url) {
        return Err("web app URL must be http or https".to_owned());
    }
    if url_host(&url).is_none() {
        return Err(format!("web app URL has no valid host: {url}"));
    }
    Ok(url)
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if port.is_some_and(|port| port.is_empty() || !port.chars().all(|digit| digit.is_ascii_digit()))
    {
        return None;
    }
    let host = host.to_ascii_lowercase();
    is_valid_host(&host).then_some(host)
}

fn url_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| format!("{scheme}://{authority}"))
}

// Plain DNS names only, matching what the bridge accepts from the preference.
fn is_valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

// Desktop Entry "string" values: escape backslash first, then control
// characters and a leading space, so a value can never start a new key line.
fn desktop_escape(value: &str) -> String {
    let mut escaped = value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n");
    if escaped.starts_with(' ') {
        escaped.replace_range(..1, "\\s");
    }
    escaped
}

// One Exec argument, double-quoted per the freedesktop Exec specification.
fn exec_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// launcher and Omarchy menu integration, installed by `omazen setup`

pub(crate) fn install_integration(paths: &RuntimePaths) -> Result<(), String> {
    let exec = desktop_escape(&exec_quote(&omazen_command(paths).to_string_lossy()));
    let install = format!(
        concat!(
            "[Desktop Entry]\n",
            "Version=1.0\n",
            "Type=Application\n",
            "Name=Install Zen Web App\n",
            "Comment=Create a web app that runs in its own Zen profile\n",
            "Keywords=zen;webapp;web app;install;add;\n",
            "Keywords[pl]=zen;webapp;aplikacja;dodaj;\n",
            "Exec=omarchy-launch-floating-terminal-with-presentation {exec} webapp install\n",
            "Icon={icon}\n",
            "Terminal=false\n"
        ),
        exec = exec,
        icon = FALLBACK_ICON
    );
    let remove = format!(
        concat!(
            "[Desktop Entry]\n",
            "Version=1.0\n",
            "Type=Application\n",
            "Name=Remove Zen Web App\n",
            "Comment=Remove a Zen web app and its profile\n",
            "Keywords=zen;webapp;web app;remove;uninstall;\n",
            "Keywords[pl]=zen;webapp;aplikacja;usuń;odinstaluj;\n",
            "Exec={exec} webapp remove\n",
            "Icon={icon}\n",
            "Terminal=false\n"
        ),
        exec = exec,
        icon = FALLBACK_ICON
    );
    install_user_bytes(
        paths,
        install.as_bytes(),
        &paths.applications_dir.join(INSTALL_LAUNCHER),
        0o644,
        &paths.integration_manifest,
    )?;
    install_user_bytes(
        paths,
        remove.as_bytes(),
        &paths.applications_dir.join(REMOVE_LAUNCHER),
        0o644,
        &paths.integration_manifest,
    )?;
    // Launchers point at the installed `omazen`; rewrite them so a reinstall
    // (or an earlier `omazen uninstall`) leaves every web app startable again,
    // and bring each profile's managed preferences up to this release.
    for app in installed_webapps(paths) {
        if let Err(error) = write_launcher(paths, &app) {
            eprintln!("WARNING: {error}");
        }
        let refreshed = write_profile_prefs(&app.profile(), app.hosts.as_deref(), app.invert)
            .and_then(|prefs| Ok(write_user_chrome(&app.profile())? || prefs));
        match refreshed {
            Ok(true) => println!(
                "Updated Zen web app settings (applied on its next start): {}",
                app.name
            ),
            Ok(false) => {}
            Err(error) => eprintln!("WARNING: {}: {error}", app.name),
        }
    }
    install_menu_block(paths)?;
    refresh_desktop_database(paths);
    Ok(())
}

/// Removes the launchers and the menu block. Web app profiles are user data
/// (sign-ins, site storage) and stay; a reinstall restores their launchers.
/// Returns true when an owned file was modified and left in place.
pub(crate) fn remove_integration(paths: &RuntimePaths) -> Result<bool, String> {
    let mut leftovers = false;
    remove_menu_block(paths)?;
    for (path, _) in manifest_entries(&paths.integration_manifest)
        .into_iter()
        .rev()
    {
        if !remove_owned_file(&paths.integration_manifest, &path, false)? {
            leftovers = true;
        }
    }
    let apps = installed_webapps(paths);
    for app in &apps {
        remove_launcher(paths, &app.slug)?;
    }
    if !apps.is_empty() {
        println!(
            "Kept {} Zen web app profile(s) in {}; reinstalling Omazen restores their launchers.",
            apps.len(),
            paths.webapps_dir.display()
        );
    }
    refresh_desktop_database(paths);
    Ok(leftovers)
}

/// Only reports once setup has installed the integration, so a fresh
/// environment's doctor output is unchanged.
pub(crate) fn doctor_integration(report: &mut DoctorReport, paths: &RuntimePaths) {
    let entries = manifest_entries(&paths.integration_manifest);
    if entries.is_empty() {
        return;
    }
    for (path, expected) in &entries {
        match sha256_file(path) {
            Ok(actual) if &actual == expected => {
                report.pass(format!("web app launcher: {}", path.display()));
            }
            Ok(_) => report.warn(format!("web app launcher was modified: {}", path.display())),
            Err(_) => report.fail(format!(
                "web app launcher missing: {}; run `omazen setup`",
                path.display()
            )),
        }
    }
    let menu = fs::read_to_string(&paths.menu_extension_file).is_ok_and(|text| {
        text.lines()
            .any(|line| line.trim_start().starts_with(MENU_BEGIN))
    });
    if menu {
        report.pass(format!(
            "Omarchy menu offers Zen web apps: {}",
            paths.menu_extension_file.display()
        ));
    } else {
        report.warn("Omarchy menu is missing the Zen web app actions; run `omazen setup`");
    }
    for app in installed_webapps(paths) {
        if is_our_launcher(&launcher_path(paths, &app.slug), &app.slug) {
            let theme = if app.themed() { " (Omarchy theme)" } else { "" };
            report.pass(format!("Zen web app: {}{theme}", app.name));
        } else {
            report.warn(format!(
                "Zen web app '{}' has no launcher; run `omazen setup`",
                app.name
            ));
        }
    }
}

fn menu_block(paths: &RuntimePaths) -> String {
    let command = shell_quote(&omazen_command(paths).to_string_lossy());
    let applications = shell_quote(&paths.applications_dir.to_string_lossy());
    let install =
        format!("omarchy-launch-floating-terminal-with-presentation {command} webapp install");
    let remove = format!("{command} webapp remove");
    let when =
        format!("grep -qsx '{LAUNCHER_MARKER}=.*' {applications}/{LAUNCHER_PREFIX}*.desktop");
    format!(
        concat!(
            "  {begin} (managed by `omazen setup`, removed by `omazen uninstall`)\n",
            "  \"install.omazen-webapp\": {{\"icon\":\"{glyph}\",\"label\":\"Zen Web App\",",
            "\"description\":\"Web app in its own Zen profile\",\"action\":\"{install}\"}},\n",
            "  \"remove.omazen-webapp\": {{\"icon\":\"{glyph}\",\"label\":\"Zen Web App\",",
            "\"description\":\"Remove a Zen web app and its profile\",",
            "\"when\":\"{when}\",\"action\":\"{remove}\"}},\n",
            "  {end}\n"
        ),
        begin = MENU_BEGIN,
        end = MENU_END,
        glyph = WEB_GLYPH,
        install = json_escape(&install),
        when = json_escape(&when),
        remove = json_escape(&remove),
    )
}

// Omarchy reads a single extension file and strips whole-line `//` comments
// and trailing commas before parsing, so the block can open the object with a
// trailing comma of its own and still leave the user's entries untouched.
fn upsert_menu_block(existing: Option<&str>, block: &str) -> Result<String, String> {
    let text = strip_menu_block(existing.unwrap_or(""))?;
    let mut output = String::with_capacity(text.len() + block.len() + 8);
    let mut inserted = false;
    for line in text.split_inclusive('\n') {
        if !inserted
            && !line.trim_start().starts_with("//")
            && let Some(index) = line.find('{')
        {
            let (head, tail) = line.split_at(index + 1);
            output.push_str(head);
            output.push('\n');
            output.push_str(block);
            let tail = tail.trim_start_matches([' ', '\t']);
            if !tail.is_empty() && tail != "\n" {
                output.push_str("  ");
                output.push_str(tail);
            }
            inserted = true;
            continue;
        }
        output.push_str(line);
    }
    if !inserted {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("{\n");
        output.push_str(block);
        output.push_str("}\n");
    }
    Ok(output)
}

fn strip_menu_block(text: &str) -> Result<String, String> {
    let mut output = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !inside && trimmed.starts_with(MENU_BEGIN) {
            inside = true;
            continue;
        }
        if inside {
            if trimmed.starts_with(MENU_END) {
                inside = false;
            }
            continue;
        }
        output.push_str(line);
    }
    if inside {
        return Err(
            "the Omazen web app block in the Omarchy menu has no end marker; fix it by hand"
                .to_owned(),
        );
    }
    Ok(output)
}

fn install_menu_block(paths: &RuntimePaths) -> Result<(), String> {
    let path = &paths.menu_extension_file;
    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let updated = upsert_menu_block(existing.as_deref(), &menu_block(paths))?;
    if existing.as_deref() == Some(updated.as_str()) {
        return Ok(());
    }
    write_preserving_mode(path, updated.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    println!(
        "Added Zen web app actions to the Omarchy menu: {}",
        path.display()
    );
    Ok(())
}

fn remove_menu_block(paths: &RuntimePaths) -> Result<(), String> {
    let path = &paths.menu_extension_file;
    let Ok(existing) = fs::read_to_string(path) else {
        return Ok(());
    };
    let stripped = strip_menu_block(&existing)?;
    if stripped != existing {
        write_preserving_mode(path, stripped.as_bytes())
            .map_err(|error| format!("{}: {error}", path.display()))?;
        println!(
            "Removed Zen web app actions from the Omarchy menu: {}",
            path.display()
        );
    }
    Ok(())
}

// The menu file is the user's own: keep its permissions, and write through a
// symlink (dotfile managers often link it) instead of replacing the link.
fn write_preserving_mode(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    let mode = fs::metadata(&target).map_or(0o644, |meta| meta.permissions().mode() & 0o7777);
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.omazen.{}.{:x}",
        target
            .file_name()
            .unwrap_or_else(|| OsStr::new("menu"))
            .to_string_lossy(),
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
        fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

// ---------------------------------------------------------------------------
// Hyprland client lookup, without a JSON dependency

fn window_address(clients: &str, class: &str) -> Option<String> {
    top_level_objects(clients).into_iter().find_map(|object| {
        if json_string_field(object, "class").as_deref() != Some(class) {
            return None;
        }
        json_string_field(object, "address").filter(|address| {
            address.len() > 2
                && address.starts_with("0x")
                && address[2..].chars().all(|digit| digit.is_ascii_hexdigit())
        })
    })
}

fn top_level_objects(json: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut start = None;
    for (index, byte) in json.bytes().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                if byte == b'{' && depth == 1 {
                    start = Some(index);
                }
                depth += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if byte == b'}'
                    && depth == 1
                    && let Some(begin) = start.take()
                {
                    objects.push(&json[begin..=index]);
                }
            }
            _ => {}
        }
    }
    objects
}

fn json_string_field(object: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after = &object[object.find(&needle)? + needle.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let body = after.strip_prefix('"')?;
    let mut value = String::new();
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(value),
            '\\' => value.push(characters.next()?),
            other => value.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_arguments_accept_flags_anywhere_before_the_separator() {
        let arguments: Vec<OsString> = ["--theme", "Mail", "mail.example.com", "--invert"]
            .iter()
            .map(OsString::from)
            .collect();
        assert_eq!(
            parse_install_arguments(&arguments),
            Ok(InstallRequest {
                name: "Mail".to_owned(),
                url: "mail.example.com".to_owned(),
                icon: None,
                theme: true,
                invert: true,
            })
        );
        let literal: Vec<OsString> = ["--", "--weird", "https://x.io"]
            .iter()
            .map(OsString::from)
            .collect();
        assert_eq!(parse_install_arguments(&literal).unwrap().name, "--weird");
        let unknown: Vec<OsString> = ["--kiosk"].iter().map(OsString::from).collect();
        assert!(parse_install_arguments(&unknown).is_err());
        let one: Vec<OsString> = ["Mail"].iter().map(OsString::from).collect();
        assert!(parse_install_arguments(&one).is_err());
    }

    #[test]
    fn profile_prefs_are_regenerated_and_user_prefs_kept() {
        let managed = managed_prefs(Some("docs.example.com"), true);
        assert!(managed.contains("user_pref(\"zen.theme.content-element-separation\", 0);"));
        assert!(managed.contains("user_pref(\"omazen.webapp.hosts\", \"docs.example.com\");"));
        let old = concat!(
            "user_pref(\"zen.view.compact.hide-tabbar\", true);\n",
            "user_pref(\"omazen.webapp.hosts\", \"stale.example.com\");\n",
            "user_pref(\"zenwebapp.theme.hosts\", \"poc.example.com\");\n",
            "  user_pref(\"layout.css.devPixelsPerPx\", \"1.25\");\n"
        );
        let merged = merge_profile_prefs(&managed, old);
        assert!(merged.starts_with(managed.trim_end()));
        assert!(merged.ends_with("user_pref(\"layout.css.devPixelsPerPx\", \"1.25\");\n"));
        assert!(merged.contains("user_pref(\"zenwebapp.theme.hosts\", \"poc.example.com\");"));
        assert!(
            !merged.contains("stale.example.com"),
            "Omazen's own preferences are regenerated"
        );
        assert_eq!(merged.matches("zen.view.compact.hide-tabbar").count(), 1);
        assert_eq!(
            merge_profile_prefs(&managed, &merged),
            merged,
            "refreshing is idempotent"
        );
        assert!(
            !managed_prefs(None, true).contains("omazen.webapp"),
            "unthemed apps get no theme prefs"
        );
        assert!(
            !managed_prefs(None, false).contains("zen.widget.linux.transparency"),
            "only themed apps are glass"
        );
    }

    #[test]
    fn user_chrome_is_managed_until_the_user_takes_it_over() {
        let profile = env::temp_dir().join(format!(
            "omazen-webapp-chrome-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let path = profile.join("chrome/userChrome.css");
        assert_eq!(
            write_user_chrome(&profile),
            Ok(true),
            "a new profile gets the file"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), USER_CHROME);
        assert_eq!(
            write_user_chrome(&profile),
            Ok(false),
            "refreshing is idempotent"
        );
        fs::write(&path, format!("/* {USER_CHROME_MARKER} */ :root {{}}\n")).unwrap();
        assert_eq!(
            write_user_chrome(&profile),
            Ok(true),
            "a stale managed file is refreshed"
        );
        fs::write(&path, ":root { --mine: 1; }\n").unwrap();
        assert_eq!(write_user_chrome(&profile), Ok(false));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            ":root { --mine: 1; }\n",
            "the user's own file is kept"
        );
        let _ = fs::remove_dir_all(&profile);
    }

    #[test]
    fn names_and_slugs() {
        assert_eq!(
            validate_name("  Google Photos "),
            Ok("Google Photos".to_owned())
        );
        assert!(validate_name("https://x.io/a").is_err());
        assert!(validate_name(" ").is_err());
        assert!(validate_name("a\nb").is_err());
        assert_eq!(slugify("Google Photos"), "google-photos");
        assert_eq!(
            slugify("  --All  Systems / Beszel!! "),
            "all-systems-beszel"
        );
        assert_eq!(slugify("Poczta Łódź"), "poczta-łódź");
        assert_eq!(slugify("!!!"), "");
        assert!(is_valid_slug("poczta-łódź"));
        assert!(!is_valid_slug("Mail"));
        assert!(!is_valid_slug("-mail"));
        assert!(!is_valid_slug("../etc"));
        assert!(!is_valid_slug(""));
    }

    #[test]
    fn urls_are_http_with_a_plain_host() {
        assert_eq!(
            normalize_url("example.com"),
            Ok("https://example.com".to_owned())
        );
        assert_eq!(
            normalize_url(" http://Localhost:3000/app "),
            Ok("http://Localhost:3000/app".to_owned())
        );
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("https://exa mple.com").is_err());
        assert!(normalize_url("https://").is_err());
        assert!(normalize_url("").is_err());
        assert_eq!(
            url_host("https://user@Mail.Example.com:8443/x?y#z"),
            Some("mail.example.com".to_owned())
        );
        assert_eq!(url_host("https://example.com:"), None);
        assert_eq!(url_host("https://ex_ample.com"), None);
        assert_eq!(url_host("https://-bad.com"), None);
        assert_eq!(
            url_origin("https://a.io:8/x/y"),
            Some("https://a.io:8".to_owned())
        );
    }

    #[test]
    fn desktop_entry_escaping() {
        assert_eq!(desktop_escape(" a\\b\tc\nd"), "\\sa\\\\b\\tc\\nd");
        assert_eq!(
            exec_quote("/home/a b/$x`\"%\\"),
            "\"/home/a b/\\$x\\`\\\"%%\\\\\""
        );
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn icon_sources() {
        assert_eq!(themed_icon_name("HEY.png"), Ok("HEY".to_owned()));
        assert_eq!(
            themed_icon_name("zen-browser"),
            Ok("zen-browser".to_owned())
        );
        assert!(themed_icon_name("a b").is_err());
        assert!(themed_icon_name("../x").is_err());
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\nrest"));
        assert!(looks_like_image(
            b"<?xml version=\"1.0\"?><svg xmlns=\"x\"/>"
        ));
        assert!(!looks_like_image(b"<!doctype html><html>"));
        let page = r#"<link rel="icon" href="/f.ico"><LINK REL='apple-touch-icon' sizes="180x180" HREF='/Touch.png'>"#;
        assert_eq!(apple_touch_icon_href(page), Some("/Touch.png".to_owned()));
        assert_eq!(apple_touch_icon_href("<link rel=icon>"), None);
        assert_eq!(resolve_href("https://a.io", "/t.png"), "https://a.io/t.png");
        assert_eq!(resolve_href("https://a.io", "t.png"), "https://a.io/t.png");
        assert_eq!(
            resolve_href("https://a.io", "//cdn.io/t.png"),
            "https://cdn.io/t.png"
        );
        assert_eq!(
            resolve_href("https://a.io", "https://b.io/t.png"),
            "https://b.io/t.png"
        );
    }

    #[test]
    fn hyprland_client_lookup() {
        let clients = r#"[
          {"address": "0x55aa", "class": "zen", "title": "a \"class\": \"omazen-webapp-mail\"",
           "workspace": {"id": 1, "name": "1"}, "grouped": [], "at": [0, 0]},
          {"address": "0x55bb", "initialClass": "omazen-webapp-mail", "class": "omazen-webapp-mail",
           "workspace": {"id": 2, "name": "2"}}
        ]"#;
        assert_eq!(
            window_address(clients, "omazen-webapp-mail"),
            Some("0x55bb".to_owned())
        );
        assert_eq!(window_address(clients, "omazen-webapp-other"), None);
        assert_eq!(
            window_address(r#"[{"class": "x", "address": "0x1\"; rm"}]"#, "x"),
            None,
            "addresses reach hyprctl only as hex"
        );
        assert_eq!(window_address("not json", "x"), None);
    }

    #[test]
    fn menu_block_is_inserted_replaced_and_removed() {
        let block = "  // >>> omazen web apps (managed)\n  \"install.omazen-webapp\": {},\n  // <<< omazen web apps\n";
        assert_eq!(
            upsert_menu_block(None, block).unwrap(),
            format!("{{\n{block}}}\n")
        );
        let user = "{\n  // Extend the menu.\n  \"personal\": {\"label\":\"Personal\"}\n}\n";
        let once = upsert_menu_block(Some(user), block).unwrap();
        assert_eq!(
            once,
            format!(
                "{{\n{block}  // Extend the menu.\n  \"personal\": {{\"label\":\"Personal\"}}\n}}\n"
            )
        );
        assert_eq!(
            upsert_menu_block(Some(&once), block).unwrap(),
            once,
            "setup is idempotent"
        );
        let replaced = upsert_menu_block(Some(&once), &block.replace("{}", "{\"x\":1}")).unwrap();
        assert_eq!(replaced.matches(MENU_BEGIN).count(), 1);
        assert!(replaced.contains("{\"x\":1}"));
        assert_eq!(
            strip_menu_block(&once).unwrap(),
            user,
            "uninstall restores the file"
        );
        let leading = "// header comment with { brace\n{\"a\":1}\n";
        assert_eq!(
            upsert_menu_block(Some(leading), block).unwrap(),
            format!("// header comment with {{ brace\n{{\n{block}  \"a\":1}}\n")
        );
        assert_eq!(
            upsert_menu_block(Some("// only a comment"), block).unwrap(),
            format!("// only a comment\n{{\n{block}}}\n")
        );
        assert!(strip_menu_block("  // >>> omazen web apps\n  \"x\": {},\n").is_err());
    }
}
