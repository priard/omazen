/* SPDX-License-Identifier: GPL-3.0-only */
/* See NOTICE for the required Omazen project attribution terms. */

import assert from "node:assert/strict";

const {
  BOOST_NAME,
  DEFAULT_STRENGTH,
  GLASS_PAGE_CSS,
  boostDataForPalette,
  createWebAppBoostDriver,
  hexToHsl,
  isInsideDirectory,
  parseHosts,
  webAppsRoot,
} = await import(new URL("../zen/Omazen/OmazenBoosts.sys.mjs", import.meta.url));

const light = { mode: "light", accent: "#56949f" };
const dark = { mode: "dark", accent: "#7aa2f7" };

// Accent mapping inverts ZenBoostsChild's hsl(dotAngleDeg, 1 - saturation, 0.1 + 0.9 * brightness).
{
  const { h, s, l } = hexToHsl(light.accent);
  const data = boostDataForPalette(light);
  assert.equal(data.boostName, BOOST_NAME);
  assert.equal(data.enableColorBoost, true);
  assert.equal(data.autoTheme, false);
  assert.equal(data.changeWasMade, true);
  assert.ok(Math.abs(data.dotAngleDeg - h) < 0.01, "hue maps to dotAngleDeg");
  assert.ok(Math.abs(1 - data.saturation - s) < 0.001, "saturation is stored inverted");
  assert.ok(Math.abs(0.1 + 0.9 * data.brightness - l) < 0.002, "brightness reproduces lightness");
  assert.equal(data.contrast, Math.round((1 - DEFAULT_STRENGTH) * 1000) / 1000);
  assert.equal(data.secondaryDotAngleDegDelta, 0);
  assert.equal(data.smartInvert, false);
  assert.equal(data.customCSS, GLASS_PAGE_CSS, "the page root is cleared for the glass window");
}
assert.equal(boostDataForPalette(dark, { invert: true }).customCSS, GLASS_PAGE_CSS, "inverted pages read on dark glass");
assert.equal(boostDataForPalette(dark).customCSS, "", "a site shown as is under a dark theme keeps its background");
assert.equal(boostDataForPalette(dark, { invert: true }).smartInvert, true, "light sites invert in dark themes");
assert.equal(boostDataForPalette(light, { invert: true }).smartInvert, false, "light themes restore the page");
assert.equal(boostDataForPalette(dark).smartInvert, false, "sites are not inverted unless marked light");
assert.equal(boostDataForPalette(light, { strength: 2 }).contrast, 0, "strength is clamped");
assert.equal(boostDataForPalette(light, { strength: Number.NaN }).contrast, 0.45, "invalid strength uses the default");
assert.equal(boostDataForPalette({ mode: "dark", accent: "red" }), null, "invalid accents never reach Zen");
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
  const prefs = { hosts, invert, strength: Number.NaN };
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
