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
export const WEBAPP_GLASS_PREF = "omazen.webapp.glass";
export const BOOST_NAME = "Omarchy theme (Omazen web app)";
export const BOOSTS_MANAGER_URI = "resource:///modules/zen/boosts/ZenBoostsManager.sys.mjs";
// A themed web app window is glass: a translucent palette layer over the
// compositor's blur. A transparent page root lets it show through; elements
// that paint their own background keep it, tinted by the boost.
export const GLASS_PAGE_CSS = "html, body { background: transparent !important; }";

const HEX_COLOR = /^#[0-9a-f]{6}$/i;
const HOST_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const SCHEDULE_DELAY_MS = 150;

// The page lightness the filter is solved for: a page's white, and its body
// text or, on a dark site, its background.
const PAGE_WHITE_L = 1;
const PAGE_DARK_L = 0.24;
// Bounds for Zen's lightness pull (see solveFilter); the upper one is Zen's
// own maximum, reached at full strength.
const MIN_PULL = 0.02;
const MAX_PULL = 0.5;
// Below this Oklab chroma a color is grey and its hue means nothing.
const NEUTRAL_CHROMA = 0.005;

const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
const round = (value, digits = 3) => Math.round(value * 10 ** digits) / 10 ** digits;
const srgbToLinear = value => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);
const linearToSrgb = value => (value <= 0.0031308 ? 12.92 * value : 1.055 * value ** (1 / 2.4) - 0.055);

export const hexToRgb = hex => [1, 3, 5].map(index => parseInt(hex.slice(index, index + 2), 16));

// The same Oklab conversion Zen's boost backend uses.
export function rgbToOklab(rgb) {
  const [lr, lg, lb] = rgb.map(channel => srgbToLinear(channel / 255));
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return {
    L: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    a: 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    b: 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  };
}

// Unclamped, so a caller can tell when a color falls outside sRGB.
export function oklabToRgb({ L, a, b }) {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map(channel => 255 * linearToSrgb(channel));
}

// Zen's smart invert flips a color's HSL lightness and keeps its hue: every
// channel moves by 255 - (max + min), so inverting twice is the identity.
export function invertLightness(rgb) {
  const shift = 255 - Math.max(...rgb) - Math.min(...rgb);
  return rgb.map(channel => channel + shift);
}

// zen.boosts.invert-channel-floor, Zen's default: a dark inverted color has
// its channels lifted from [0, 255] into [floor, 255].
const INVERT_CHANNEL_FLOOR = 15;

// The color the filter must produce so that Zen's invert yields `rgb`.
export function preInvert(rgb) {
  const luma = (rgb[0] * 54 + rgb[1] * 183 + rgb[2] * 19) >> 8;
  const unlifted =
    luma > 127
      ? rgb
      : rgb.map(channel =>
          clamp(
            Math.round(((channel - INVERT_CHANNEL_FLOOR) * 255) / (255 - INVERT_CHANNEL_FLOOR)),
            0,
            255,
          ),
        );
  return invertLightness(unlifted);
}

export function hexToHsl(hex) {
  return rgbToHsl(hexToRgb(hex));
}

function rgbToHsl(rgb) {
  const [r, g, b] = rgb.map(channel => channel / 255);
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

// Zen's boost is a duotone filter in Oklab. Each page color is pulled toward
// one of two accents by its lightness: dark colors toward the accent, light
// ones toward a complement that has the accent's lightness and chroma and a
// rotated hue. The strength s blends the color's chroma toward the selected
// accent by s, moves its lightness toward the accent's by s² / 2 (the pull),
// and then turns its hue by s² / 4 radians. solveFilter picks the accent, the
// rotation and the strength so that a page's white lands on `lightTarget` and
// its dark anchor on `darkTarget`. Only one chroma is available, so it is
// taken from the favored side, which is the one that becomes the theme's
// background.
export function solveFilter(lightTarget, darkTarget, favorLight) {
  const light = rgbToOklab(lightTarget);
  const dark = rgbToOklab(darkTarget);
  // Anchor x lands on x + pull * (accentL - x). Solving both anchors exactly
  // gives the pull from their spread and the accent lightness from either.
  const span = PAGE_WHITE_L - PAGE_DARK_L;
  let pull = clamp(1 - (light.L - dark.L) / span, MIN_PULL, MAX_PULL);
  const exactL =
    (light.L + dark.L - PAGE_WHITE_L - PAGE_DARK_L) / (2 * pull) + (PAGE_WHITE_L + PAGE_DARK_L) / 2;
  // ZenBoostsChild never builds an accent below HSL lightness 0.1, which is
  // no darker than the grey #1a1a1a.
  const accentL = clamp(exactL, rgbToOklab([26, 26, 26]).L, 0.98);
  // When no real accent is that light or dark, fit the pull again, by least
  // squares, for the lightness that is left.
  if (accentL !== exactL) {
    const [white, darkAnchor] = [PAGE_WHITE_L - accentL, PAGE_DARK_L - accentL];
    pull = clamp(
      ((PAGE_WHITE_L - light.L) * white + (PAGE_DARK_L - dark.L) * darkAnchor) /
        (white * white + darkAnchor * darkAnchor),
      MIN_PULL,
      MAX_PULL,
    );
  }
  const strength = Math.sqrt(2 * pull);
  // The final hue turn follows the direction of the lightness shift.
  const twist = anchorL => ((accentL < anchorL ? 1 : -1) * pull) / 2;
  const chroma = color => Math.hypot(color.a, color.b);
  const hue = color => Math.atan2(color.b, color.a);
  const [main, other] = favorLight ? [light, dark] : [dark, light];
  const otherHue = chroma(other) < NEUTRAL_CHROMA ? hue(main) : hue(other);
  const lightHue = (favorLight ? hue(main) : otherHue) - twist(PAGE_WHITE_L);
  const darkHue = (favorLight ? otherHue : hue(main)) - twist(PAGE_DARK_L);
  const accentAt = amount =>
    oklabToRgb({ L: accentL, a: amount * Math.cos(darkHue), b: amount * Math.sin(darkHue) });
  const inGamut = rgb => rgb.every(channel => channel >= -0.5 && channel <= 255.5);
  let amount = chroma(main) / strength;
  if (!inGamut(accentAt(amount))) {
    let [low, high] = [0, amount];
    for (let step = 0; step < 24; step += 1) {
      const middle = (low + high) / 2;
      if (inGamut(accentAt(middle))) low = middle;
      else high = middle;
    }
    amount = low;
  }
  let rotation = ((lightHue - darkHue) * 180) / Math.PI;
  rotation = ((((rotation + 180) % 360) + 360) % 360) - 180;
  return {
    accent: accentAt(amount).map(channel => clamp(Math.round(channel), 0, 255)),
    strength,
    rotation,
  };
}

export function boostDataForPalette(palette, { invert = false, glass = true } = {}) {
  if (!palette || !["background", "foreground"].every(key => HEX_COLOR.test(palette[key] ?? ""))) {
    return null;
  }
  const background = hexToRgb(palette.background);
  const foreground = hexToRgb(palette.foreground);
  // --invert marks a light site: invert it only while the theme is dark, so
  // a light theme restores the page instead of leaving it dark.
  const smartInvert = Boolean(invert) && palette.mode === "dark";
  // Under a light theme a page's white becomes the theme background and its
  // text the foreground. Zen inverts after filtering, so an inverted page aims
  // at the colors that invert to them. A page shown as is under a dark theme
  // gets the dark scheme, so its dark background becomes the theme background
  // and its light text the foreground.
  const { accent, strength, rotation } =
    palette.mode !== "dark"
      ? solveFilter(background, foreground, true)
      : smartInvert
        ? solveFilter(preInvert(background), preInvert(foreground), true)
        : solveFilter(foreground, background, false);
  const { h, s, l } = rgbToHsl(accent);
  return {
    boostName: BOOST_NAME,
    enableColorBoost: true,
    autoTheme: false,
    // ZenBoostsChild builds the accent as hsl(dotAngleDeg, 1 - saturation,
    // 0.1 + 0.9 * brightness), with (1 - contrast) * 255 in its alpha byte as
    // the strength.
    dotAngleDeg: round(h, 2),
    saturation: round(1 - s),
    brightness: round(clamp((l - 0.1) / 0.9, 0, 1)),
    contrast: round(1 - strength),
    secondaryDotAngleDegDelta: round(rotation, 2),
    smartInvert,
    // On a glass window the page root is cleared where its text stays
    // readable: dark text on a light theme, or inverted light text on a dark
    // one. A site shown as is under a dark theme may only have dark text, so
    // it keeps its own background, as every page does in an opaque window.
    customCSS: glass && (palette.mode === "light" || smartInvert) ? GLASS_PAGE_CSS : "",
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
    const { hosts, invert, glass } = readPrefs();
    const desired = enabled ? boostDataForPalette(palette, { invert, glass }) : null;
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
      glass: prefs.getBoolPref(WEBAPP_GLASS_PREF, true),
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
