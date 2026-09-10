/* SPDX-License-Identifier: GPL-3.0-only */
/* See NOTICE for the required Omazen project attribution terms. */

"use strict";

// Optional page theming for Omazen web apps.
//
// `omazen webapp install --theme` records the hosts a web app serves in that
// web app's own Zen profile. In that profile only, this module turns the
// active Omarchy palette into a Zen boost for those hosts, so the page follows
// theme switches the way the browser chrome does. Everywhere else it stays
// inert: the bridge starts it only when the profile directory lies inside the
// Omazen web apps directory *and* carries the hosts preference. A regular Zen
// profile never meets both conditions, so ordinary browsing is not recolored.
//
// Zen's color boost is a duotone tint rather than a palette replacement: dark
// page colors lean toward the accent and light ones toward its hue-rotated
// complement, with lightness shifted at most halfway. Inversion, used for light
// sites under dark themes, is Zen's own and leaves images alone.

export const WEBAPP_HOSTS_PREF = "omazen.webapp.hosts";
export const WEBAPP_INVERT_PREF = "omazen.webapp.invert";
export const WEBAPP_STRENGTH_PREF = "omazen.webapp.strength";
export const BOOST_NAME = "Omarchy theme (Omazen web app)";
export const DEFAULT_STRENGTH = 0.55;
export const BOOSTS_MANAGER_URI = "resource:///modules/zen/boosts/ZenBoostsManager.sys.mjs";
// A themed web app window is glass: a translucent palette layer over the
// compositor's blur. A transparent page root lets it show through; elements
// that paint their own background keep it, tinted by the boost.
export const GLASS_PAGE_CSS = "html, body { background: transparent !important; }";

const HEX_COLOR = /^#[0-9a-f]{6}$/i;
const HOST_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const SCHEDULE_DELAY_MS = 150;

const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
const round = (value, digits = 3) => Math.round(value * 10 ** digits) / 10 ** digits;

export function hexToHsl(hex) {
  const [r, g, b] = [1, 3, 5].map(index => parseInt(hex.slice(index, index + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return { h: 0, s: 0, l };
  const delta = max - min;
  const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min);
  let h;
  if (max === r) h = (g - b) / delta + (g < b ? 6 : 0);
  else if (max === g) h = (b - r) / delta + 2;
  else h = (r - g) / delta + 4;
  return { h: h * 60, s, l };
}

// Hosts are written by the CLI, but the preference is user-editable, so only
// plain DNS names reach Zen's boost store.
export function parseHosts(value) {
  const hosts = [];
  for (const raw of String(value ?? "").split(",")) {
    const host = raw.trim().toLowerCase();
    if (!host || host.length > 253 || hosts.includes(host)) continue;
    if (host.split(".").every(label => HOST_LABEL.test(label))) hosts.push(host);
  }
  return hosts;
}

// Mirrors the CLI's default: $OMAZEN_WEBAPPS_DIR, else <data home>/omazen-webapps.
export function webAppsRoot(env, home) {
  const configured = env("OMAZEN_WEBAPPS_DIR");
  if (configured) return configured;
  const share = env("OMAZEN_SHARE_DIR") || env("XDG_DATA_HOME") || `${home}/.local/share`;
  return `${share}/omazen-webapps`;
}

export function isInsideDirectory(path, directory) {
  if (typeof path !== "string" || typeof directory !== "string") return false;
  const root = directory.replace(/\/+$/, "");
  return root !== "" && path.startsWith(`${root}/`);
}

export function boostDataForPalette(palette, { invert = false, strength = DEFAULT_STRENGTH } = {}) {
  if (!palette || !HEX_COLOR.test(palette.accent ?? "")) return null;
  const { h, s, l } = hexToHsl(palette.accent);
  const amount = Number.isFinite(strength) ? clamp(strength, 0, 1) : DEFAULT_STRENGTH;
  // --invert marks a light site: invert it only while the theme is dark, so
  // a light theme restores the page instead of leaving it dark.
  const smartInvert = Boolean(invert) && palette.mode === "dark";
  return {
    boostName: BOOST_NAME,
    enableColorBoost: true,
    autoTheme: false,
    // ZenBoostsChild builds the accent as hsl(dotAngleDeg, 1 - saturation,
    // 0.1 + 0.9 * brightness); invert that mapping to land on the palette accent.
    dotAngleDeg: round(h, 2),
    saturation: round(1 - s),
    brightness: round(clamp((l - 0.1) / 0.9, 0, 1)),
    // The accent's alpha byte carries (1 - contrast) * 255, which the backend
    // uses as its blend factor, so contrast = 1 - strength.
    contrast: round(1 - amount),
    // No rotation keeps light and dark page colors on the palette's own hue.
    secondaryDotAngleDegDelta: 0,
    smartInvert,
    // The page root is cleared only where its text stays readable on the
    // glass: dark text on a light theme, or inverted light text on a dark one.
    // A site shown as is under a dark theme may only have dark text, so it
    // keeps its own background.
    customCSS: palette.mode === "light" || smartInvert ? GLASS_PAGE_CSS : "",
    changeWasMade: true,
  };
}

// The driver owns at most one boost per host, recognised by its name, and
// leaves any boost the user made in Zen's editor alone.
export function createWebAppBoostDriver({
  manager,
  readPrefs,
  addObserver,
  setTimer,
  clearTimer,
  log = () => {},
}) {
  let palette = null;
  let enabled = null;
  let applying = false;
  let timer = null;

  const ownedBoost = host =>
    (manager.loadBoostsFromStore(host) || []).find(
      boost => boost?.boostEntry?.boostData?.boostName === BOOST_NAME,
    ) || null;
  const matches = (data, desired) => Object.keys(desired).every(key => data?.[key] === desired[key]);

  function apply() {
    if (applying || enabled === null) return;
    const { hosts, invert, strength } = readPrefs();
    const desired = enabled ? boostDataForPalette(palette, { invert, strength }) : null;
    if (enabled && !desired) return;
    applying = true;
    try {
      const changed = [];
      for (const host of hosts) {
        const boost = ownedBoost(host);
        if (!desired) {
          if (boost) {
            manager.deleteBoost(boost);
            changed.push(host);
          }
          continue;
        }
        if (
          boost &&
          manager.getActiveBoostId(host) === boost.id &&
          matches(boost.boostEntry.boostData, desired)
        ) {
          continue;
        }
        const target = boost || manager.createNewBoost(host);
        if (!target) continue;
        Object.assign(target.boostEntry.boostData, desired);
        manager.saveBoostToStore(target);
        if (manager.getActiveBoostId(host) !== target.id) {
          manager.makeBoostActiveForDomain(host, target.id);
        }
        changed.push(host);
      }
      if (changed.length) {
        log(
          desired
            ? `WEBAPP_BOOST_APPLIED hosts=${changed.join(",")} accent=${palette.accent} invert=${desired.smartInvert}`
            : `WEBAPP_BOOST_REMOVED hosts=${changed.join(",")}`,
        );
      }
    } catch (error) {
      // Zen reads zen-boosts.jsonlz4 asynchronously, and saving before that
      // load finishes throws. The load ends with zen-boosts-update, which
      // schedules another attempt, so this is expected rather than a failure.
      log(`WEBAPP_BOOST_PENDING reason=${String(error?.message ?? error).replace(/\s+/g, " ")}`);
    } finally {
      applying = false;
    }
  }

  // Zen notifies zen-boosts-update synchronously from inside its own save, so
  // reacting in place would re-enter apply() mid-write. Every trigger goes
  // through a short timer instead; apply() is idempotent, so our own saves
  // settle into a no-op.
  function schedule(delay = SCHEDULE_DELAY_MS) {
    if (timer !== null) clearTimer(timer);
    timer = setTimer(() => {
      timer = null;
      apply();
    }, delay);
  }

  let removeObserver = addObserver("zen-boosts-update", () => schedule());

  return {
    update(nextPalette, nextEnabled) {
      if (nextPalette) palette = nextPalette;
      enabled = Boolean(nextEnabled);
      schedule();
    },
    dispose() {
      if (timer !== null) clearTimer(timer);
      timer = null;
      removeObserver?.();
      removeObserver = null;
    },
  };
}

let sharedDriver = null;

// Returns the process-wide driver for an Omazen web app profile, or null for
// every other profile. Each browser window's bridge calls this; the first call
// creates the driver and the rest share it. `profileDir` may be a function so
// regular profiles, which fail the preference check, never have to resolve it.
export function startWebAppBoosts({ profileDir, env, home, log } = {}) {
  if (sharedDriver) return sharedDriver;
  const prefs = Services.prefs;
  if (!parseHosts(prefs.getStringPref(WEBAPP_HOSTS_PREF, "")).length) return null;
  const directory = typeof profileDir === "function" ? profileDir() : profileDir;
  if (!isInsideDirectory(directory, webAppsRoot(env, home))) return null;
  const { gZenBoostsManager } = ChromeUtils.importESModule(BOOSTS_MANAGER_URI);
  const { setTimeout, clearTimeout } = ChromeUtils.importESModule(
    "resource://gre/modules/Timer.sys.mjs",
  );
  sharedDriver = createWebAppBoostDriver({
    manager: gZenBoostsManager,
    readPrefs: () => ({
      hosts: parseHosts(prefs.getStringPref(WEBAPP_HOSTS_PREF, "")),
      invert: prefs.getBoolPref(WEBAPP_INVERT_PREF, false),
      strength: Number.parseFloat(prefs.getStringPref(WEBAPP_STRENGTH_PREF, "")),
    }),
    addObserver: (topic, callback) => {
      const observer = { observe: () => callback() };
      Services.obs.addObserver(observer, topic);
      return () => Services.obs.removeObserver(observer, topic);
    },
    setTimer: setTimeout,
    clearTimer: clearTimeout,
    // The window whose bridge created the driver may close before the process
    // ends; logging must never take the driver down with it.
    log: message => {
      try {
        log?.(message);
      } catch {}
    },
  });
  return sharedDriver;
}
