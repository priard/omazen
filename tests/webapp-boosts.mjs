/* SPDX-License-Identifier: GPL-3.0-only */
/* See NOTICE for the required Omazen project attribution terms. */

import assert from "node:assert/strict";

const {
  BOOST_NAME,
  GLASS_PAGE_CSS,
  boostDataForPalette,
  createWebAppBoostDriver,
  hexToHsl,
  hexToRgb,
  invertLightness,
  isInsideDirectory,
  rgbToOklab,
  parseHosts,
  webAppsRoot,
} = await import(new URL("../zen/Omazen/OmazenBoosts.sys.mjs", import.meta.url));

const light = { mode: "light", accent: "#17769e", background: "#fceeea", foreground: "#3c383e" };
const dark = { mode: "dark", accent: "#7aa2f7", background: "#1a1b26", foreground: "#c0caf5" };

// A port of what Zen does with the boost data: ZenBoostsChild builds the
// accent, and nsZenBoostsBackend.cpp filters (and optionally inverts) colors.
function zenRender(data, rgb) {
  const hueToRgb = (p, q, t) => {
    t = t < 0 ? t + 1 : t > 1 ? t - 1 : t;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    return t < 2 / 3 ? p + (q - p) * (2 / 3 - t) * 6 : p;
  };
  const [h, s, l] = [data.dotAngleDeg / 360, 1 - data.saturation, 0.1 + 0.9 * data.brightness];
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const accentRgb = s === 0 ? [l, l, l].map(v => Math.round(v * 255))
    : [h + 1 / 3, h, h - 1 / 3].map(t => Math.round(hueToRgb(2 * l - q, q, t) * 255));
  const blend = (((1 - data.contrast) * 255) << 0) / 255;
  const accent = rgbToOklab(accentRgb);
  const turn = (a, b, angle) => [a * Math.cos(angle) - b * Math.sin(angle), a * Math.sin(angle) + b * Math.cos(angle)];
  const [compA, compB] = turn(accent.a, accent.b, (data.secondaryDotAngleDegDelta * Math.PI) / 180);
  const original = rgbToOklab(rgb);
  const halfWidth = Math.min(0.5, Math.max(0.05, 0.5 - blend * 0.45));
  let t = Math.min(1, Math.max(0, (original.L - (0.5 - halfWidth)) / (2 * halfWidth)));
  t = t * t * (3 - 2 * t);
  const delta = accent.L - original.L;
  const [a, b] = turn(
    original.a + (accent.a + (compA - accent.a) * t - original.a) * blend,
    original.b + (accent.b + (compB - accent.b) * t - original.b) * blend,
    (delta > 0 ? -1 : 1) * blend * blend * 0.25,
  );
  const L = original.L + delta * blend * blend * 0.5;
  const lms = [
    L + 0.3963377774 * a + 0.2158037573 * b,
    L - 0.1055613458 * a - 0.0638541728 * b,
    L - 0.0894841775 * a - 1.291485548 * b,
  ].map(v => v ** 3);
  let out = [
    4.0767416621 * lms[0] - 3.3077115913 * lms[1] + 0.2309699292 * lms[2],
    -1.2684380046 * lms[0] + 2.6097574011 * lms[1] - 0.3413193965 * lms[2],
    -0.0041960863 * lms[0] - 0.7034186147 * lms[1] + 1.707614701 * lms[2],
  ].map(v => {
    const c = Math.max(0, v);
    const srgb = c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
    return Math.min(255, Math.max(0, Math.floor(srgb * 255 + 0.5)));
  });
  if (data.smartInvert) {
    out = invertLightness(out);
    if (((out[0] * 54 + out[1] * 183 + out[2] * 19) >> 8) <= 127) {
      out = out.map(c => Math.floor(15 + (c * 240) / 255));
    }
  }
  return out;
}
const distance = (x, y) => {
  const [p, q] = [rgbToOklab(x), rgbToOklab(y)];
  return Math.hypot(p.L - q.L, p.a - q.a, p.b - q.b);
};

// The boost is solved from the palette: a page's white becomes the theme
// background and its text the foreground, also through Zen's inversion.
{
  const data = boostDataForPalette(light);
  assert.equal(data.boostName, BOOST_NAME);
  assert.equal(data.enableColorBoost, true);
  assert.equal(data.autoTheme, false);
  assert.equal(data.changeWasMade, true);
  assert.equal(data.smartInvert, false);
  assert.equal(data.customCSS, GLASS_PAGE_CSS, "the page root is cleared for the glass window");
  assert.ok(distance(zenRender(data, [255, 255, 255]), hexToRgb(light.background)) < 0.005,
    "white pages take the theme background");
  assert.ok(distance(zenRender(data, [34, 34, 34]), hexToRgb(light.foreground)) < 0.02,
    "text takes the theme foreground");
}
{
  const data = boostDataForPalette(dark, { invert: true });
  assert.ok(distance(zenRender(data, [255, 255, 255]), hexToRgb(dark.background)) < 0.005,
    "an inverted white page takes the dark theme background");
  assert.ok(distance(zenRender(data, [34, 34, 34]), hexToRgb(dark.foreground)) < 0.08,
    "inverted text stays near the dark theme foreground");
}
{
  const data = boostDataForPalette(dark);
  assert.ok(distance(zenRender(data, [30, 30, 30]), hexToRgb(dark.background)) < 0.01,
    "a dark site's background takes the dark theme background");
  assert.ok(distance(zenRender(data, [232, 232, 232]), hexToRgb(dark.foreground)) < 0.08,
    "a dark site's text stays near the dark theme foreground");
}
for (const data of [boostDataForPalette(light), boostDataForPalette(dark, { invert: true }), boostDataForPalette(dark)]) {
  for (const key of ["saturation", "brightness", "contrast"]) {
    assert.ok(data[key] >= 0 && data[key] <= 1, `${key} stays within Zen's range`);
  }
  assert.ok(Math.abs(data.secondaryDotAngleDegDelta) <= 180);
}
assert.equal(boostDataForPalette(dark, { invert: true }).customCSS, GLASS_PAGE_CSS, "inverted pages read on dark glass");
assert.equal(boostDataForPalette(dark).customCSS, "", "a site shown as is under a dark theme keeps its background");
assert.equal(boostDataForPalette(dark, { invert: true }).smartInvert, true, "light sites invert in dark themes");
assert.equal(boostDataForPalette(light, { invert: true }).smartInvert, false, "light themes restore the page");
assert.equal(boostDataForPalette(dark).smartInvert, false, "sites are not inverted unless marked light");
assert.equal(boostDataForPalette({ ...dark, background: "red" }), null, "invalid colors never reach Zen");
assert.equal(boostDataForPalette(null), null);
assert.deepEqual(hexToHsl("#808080"), { h: 0, s: 0, l: 128 / 255 });

assert.deepEqual(
  parseHosts(" Mail.Example.com ,mail.example.com,bad host,-x.com,x-.com,a..b,ok-1.io,\"q\".com,"),
  ["mail.example.com", "ok-1.io"],
  "hosts are normalized, de-duplicated and restricted to DNS names",
);
assert.deepEqual(parseHosts(undefined), []);

assert.equal(isInsideDirectory("/h/.local/share/omazen-webapps/mail/profile", "/h/.local/share/omazen-webapps"), true);
assert.equal(isInsideDirectory("/h/.local/share/omazen-webapps/mail/profile", "/h/.local/share/omazen-webapps/"), true);
assert.equal(isInsideDirectory("/h/.local/share/omazen-webapps-evil/profile", "/h/.local/share/omazen-webapps"), false);
assert.equal(isInsideDirectory("/h/.config/zen/abc.Default", "/h/.local/share/omazen-webapps"), false);
assert.equal(isInsideDirectory("/anything", "/"), false, "an empty root never matches");
assert.equal(isInsideDirectory(undefined, "/h"), false);

const env = values => name => values[name] ?? "";
assert.equal(webAppsRoot(env({}), "/h"), "/h/.local/share/omazen-webapps");
assert.equal(webAppsRoot(env({ XDG_DATA_HOME: "/x" }), "/h"), "/x/omazen-webapps");
assert.equal(webAppsRoot(env({ XDG_DATA_HOME: "/x", OMAZEN_SHARE_DIR: "/s" }), "/h"), "/s/omazen-webapps");
assert.equal(webAppsRoot(env({ OMAZEN_WEBAPPS_DIR: "/w", OMAZEN_SHARE_DIR: "/s" }), "/h"), "/w");

// A manager double that behaves like Zen's: it notifies synchronously from
// inside its own saves and throws when written before its store has loaded.
function createFakeManager({ ready = true } = {}) {
  const domains = new Map();
  const listeners = new Set();
  const calls = [];
  let nextId = 1;
  const notify = () => {
    for (const listener of [...listeners]) listener();
  };
  const domain = host => {
    if (!domains.has(host)) domains.set(host, { active: null, boosts: new Map() });
    return domains.get(host);
  };
  const manager = {
    ready,
    loadBoostsFromStore(host) {
      const entry = domains.get(host);
      return entry ? [...entry.boosts].map(([id, boostEntry]) => ({ id, domain: host, boostEntry })) : null;
    },
    createNewBoost(host) {
      const id = `boost-${nextId++}`;
      const boostEntry = { boostData: { boostName: "My Boost", changeWasMade: false } };
      domain(host).boosts.set(id, boostEntry);
      return { id, domain: host, boostEntry };
    },
    saveBoostToStore(boost) {
      if (!manager.ready) throw new TypeError("can't access property \"data\", this.#file is null");
      calls.push(["save", boost.domain]);
      domain(boost.domain).boosts.set(boost.id, boost.boostEntry);
      notify();
    },
    getActiveBoostId(host) {
      return domains.get(host)?.active ?? null;
    },
    makeBoostActiveForDomain(host, id) {
      calls.push(["activate", host]);
      domain(host).active = id;
      notify();
    },
    deleteBoost(boost) {
      calls.push(["delete", boost.domain]);
      const entry = domain(boost.domain);
      entry.boosts.delete(boost.id);
      if (entry.active === boost.id) entry.active = null;
      notify();
    },
  };
  return { manager, domains, calls, listeners, notify };
}

function createHarness({ hosts = ["mail.example.com"], invert = false, ready = true } = {}) {
  const fake = createFakeManager({ ready });
  const timers = new Map();
  let nextTimer = 1;
  const logs = [];
  const prefs = { hosts, invert };
  const driver = createWebAppBoostDriver({
    manager: fake.manager,
    readPrefs: () => ({ ...prefs }),
    addObserver: (topic, callback) => {
      assert.equal(topic, "zen-boosts-update");
      fake.listeners.add(callback);
      return () => fake.listeners.delete(callback);
    },
    setTimer: (callback, delay) => {
      assert.ok(delay > 0, "reactions are deferred, never run inside Zen's notification");
      const id = nextTimer++;
      timers.set(id, callback);
      return id;
    },
    clearTimer: id => timers.delete(id),
    log: message => logs.push(message),
  });
  const runTimers = () => {
    let guard = 0;
    while (timers.size) {
      assert.ok(++guard < 20, "timers settle instead of looping on our own saves");
      const [id, callback] = timers.entries().next().value;
      timers.delete(id);
      callback();
    }
  };
  return { ...fake, driver, prefs, logs, timers, runTimers };
}

const owned = (harness, host) =>
  harness.manager.loadBoostsFromStore(host)?.find(boost => boost.boostEntry.boostData.boostName === BOOST_NAME);

// Apply creates one active boost per host and then settles.
{
  const harness = createHarness({ hosts: ["mail.example.com", "accounts.example.com"] });
  harness.driver.update(dark, true);
  assert.equal(harness.calls.length, 0, "nothing is written before the timer fires");
  harness.runTimers();
  for (const host of ["mail.example.com", "accounts.example.com"]) {
    const boost = owned(harness, host);
    assert.ok(boost, `boost created for ${host}`);
    assert.equal(harness.manager.getActiveBoostId(host), boost.id, "the boost is active");
    assert.equal(boost.boostEntry.boostData.dotAngleDeg, boostDataForPalette(dark).dotAngleDeg);
  }
  const writes = harness.calls.length;
  harness.notify();
  harness.runTimers();
  assert.equal(harness.calls.length, writes, "an unchanged palette is a no-op");
  assert.match(harness.logs.at(-1), /^WEBAPP_BOOST_APPLIED hosts=mail\.example\.com,accounts\.example\.com accent=#7aa2f7 invert=false$/);
}

// A palette change updates the same boost in place; invert follows the theme mode.
{
  const harness = createHarness({ invert: true });
  harness.driver.update(dark, true);
  harness.runTimers();
  const first = owned(harness, "mail.example.com");
  assert.equal(first.boostEntry.boostData.smartInvert, true);
  harness.driver.update(light, true);
  harness.runTimers();
  const second = owned(harness, "mail.example.com");
  assert.equal(second.id, first.id, "the boost is reused, not duplicated");
  assert.equal(harness.manager.loadBoostsFromStore("mail.example.com").length, 1);
  assert.equal(second.boostEntry.boostData.smartInvert, false, "light themes restore the page");
  assert.equal(second.boostEntry.boostData.dotAngleDeg, boostDataForPalette(light).dotAngleDeg);
}

// Disabling Omazen removes only the Omazen boost; a user's own boost survives.
{
  const harness = createHarness();
  const mine = harness.manager.createNewBoost("mail.example.com");
  mine.boostEntry.boostData.boostName = "Reading";
  harness.driver.update(dark, true);
  harness.runTimers();
  harness.driver.update(null, false);
  harness.runTimers();
  assert.equal(owned(harness, "mail.example.com"), undefined, "the Omazen boost is removed");
  assert.equal(harness.manager.loadBoostsFromStore("mail.example.com").length, 1, "the user's boost is kept");
  assert.match(harness.logs.at(-1), /^WEBAPP_BOOST_REMOVED hosts=mail\.example\.com$/);
  const writes = harness.calls.length;
  harness.driver.update(null, false);
  harness.runTimers();
  assert.equal(harness.calls.length, writes, "disabling twice is a no-op");
}

// Before Zen has loaded its store, writes throw; the load's notification retries.
{
  const harness = createHarness({ ready: false });
  harness.driver.update(dark, true);
  harness.runTimers();
  assert.equal(owned(harness, "mail.example.com")?.boostEntry.boostData.changeWasMade ?? false, true,
    "the boost data was prepared");
  assert.match(harness.logs.at(-1), /^WEBAPP_BOOST_PENDING reason=/, "the retry is logged, not raised");
  assert.ok(!harness.logs.some(line => line.includes("ERROR")), "nothing is reported as a bridge error");
  harness.manager.ready = true;
  harness.notify();
  harness.runTimers();
  assert.equal(harness.manager.getActiveBoostId("mail.example.com"), owned(harness, "mail.example.com").id);
  assert.match(harness.logs.at(-1), /^WEBAPP_BOOST_APPLIED /);
}

// No hosts, no enabled state yet, or no valid palette: nothing is written.
{
  const harness = createHarness({ hosts: [] });
  harness.driver.update(dark, true);
  harness.runTimers();
  assert.equal(harness.calls.length, 0);
}
{
  const harness = createHarness();
  harness.notify();
  harness.runTimers();
  assert.equal(harness.calls.length, 0, "notifications before the first palette do nothing");
  harness.driver.update({ mode: "dark", accent: "nope" }, true);
  harness.runTimers();
  assert.equal(harness.calls.length, 0);
}

// Dispose clears the pending timer and the observer.
{
  const harness = createHarness();
  harness.driver.update(dark, true);
  harness.driver.dispose();
  assert.equal(harness.timers.size, 0);
  assert.equal(harness.listeners.size, 0);
}

console.log("Web app boost regressions passed.");
