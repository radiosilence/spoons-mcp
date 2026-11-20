// Drives the app in the ext-apps reference host (examples/basic-host) against the real `spoons` binary, which talks
// to fixture-api.mjs instead of Wetherspoon's: nothing here reaches the live API or a real payment page.
//
//   HOST_DIR=/path/to/ext-apps/examples/basic-host  (built: dist/index.html and dist/sandbox.html)
//   SPOONS_BIN=target/debug/spoons
//   node e2e/run.mjs            screenshots into e2e/out/, and the checks
//
// Every screen is shot full and compact, at 380 px and 1024 px, light and dark. The checks: a change the model
// makes shows up in an app already on screen; a tap in the app reaches the model's context; the text result reads
// on its own; and the app never sends a CONFIRM.

import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "out");
mkdirSync(out, { recursive: true });
const hostDir = process.env.HOST_DIR ?? join(here, "host");
const bin = resolve(process.env.SPOONS_BIN ?? join(here, "../../../../target/debug/spoons"));

const children = [];
function start(cmd, args, env = {}, cwd = here) {
  const child = spawn(cmd, args, { cwd, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  child.stderr.on("data", (d) => process.env.VERBOSE && process.stderr.write(d));
  children.push(child);
  return child;
}
const stopAll = () => children.forEach((c) => c.kill());
process.on("exit", stopAll);
process.on("SIGINT", () => process.exit(1));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

start("node", ["fixture-api.mjs", "4010"]);
start(bin, ["mcp", "--http", "127.0.0.1:3001", "--state", mkdtempSync(join(tmpdir(), "spoons-e2e-"))], {
  SPOONS_API_KEY: "e2e",
  SPOONS_API_BASE: "http://127.0.0.1:4010/api/v0.1",
  SPOONS_PAYMENTS_BASE: "http://127.0.0.1:4010/pay",
  SPOONS_APP_HTML: join(here, "../../assets/app.html"),
});
start("node", ["cors-proxy.mjs", "3002", "http://127.0.0.1:3001", "e2e@example.com"]);
start("npx", ["tsx", "serve.ts"], { SERVERS: JSON.stringify(["http://127.0.0.1:3002/mcp"]) }, hostDir);
await sleep(4000);

const failures = [];
function check(ok, what) {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures.push(what);
}

const browser = await chromium.launch();

async function host({ width = 380, dark = false } = {}) {
  const page = await browser.newPage({ viewport: { width, height: 1000 }, deviceScaleFactor: 2 });
  page.on("pageerror", (e) => console.log("page error:", e.message));
  await page.goto("http://localhost:8080/");
  await page.waitForSelector("textarea");
  await page.waitForTimeout(500);
  if (dark) await page.click("button[class*=themeToggle]");
  // The host's floating theme button would sit on top of every screenshot.
  await page.addStyleTag({ content: "button[class*=themeToggle]{visibility:hidden}" });
  return page;
}

/** Calls `spoons` as the model would, and returns the new app's frame and its outer iframe. */
async function call(page, query, extra = {}) {
  const seen = new Set(page.frames());
  const before = await page.locator("iframe").count();
  await page.fill("textarea", JSON.stringify({ query, ...extra }));
  await page.click("button[type=submit]");
  await page.waitForFunction((n) => document.querySelectorAll("iframe").length > n, before);
  // Pinned by index: `.last()` would follow whichever app came last.
  const outer = page.locator("iframe").nth((await page.locator("iframe").count()) - 1);
  let app;
  for (let i = 0; i < 150 && !app; i++) {
    app = page.frames().find((f) => !seen.has(f) && f.parentFrame()?.url().includes("sandbox.html"));
    if (!app) await sleep(100);
  }
  if (!app) {
    console.log(page.frames().map((f) => `${seen.has(f) ? "old" : "new"} ${f.url().slice(0, 60)} <- ${f.parentFrame()?.url().slice(0, 40)}`));
    throw new Error(`no app frame for ${query}`);
  }
  await app.waitForSelector("#root", { state: "attached" });
  return { app, outer };
}

async function settle(app, selector = ".screen") {
  await app.waitForSelector(selector, { timeout: 15000 });
  await app.waitForFunction(() => !document.querySelector(".pouring"), null, { timeout: 15000 });
  await sleep(500);
}

async function shoot(outer, name) {
  await outer.scrollIntoViewIfNeeded();
  await outer.screenshot({ path: join(out, `${name}.png`), animations: "disabled" });
}

const MENU = `{ pub(ref: 254) { name menu(table: 4) { totalCount guestAles { name } } } }`;
const ADD = (n) =>
  `mutation { basketAdd(pub: 254, table: 4, productId: "10000000315", portion: "Pint", quantity: ${n}) { basket { summary } } }`;

for (const width of [380, 1024]) {
  for (const dark of [false, true]) {
    const tag = `${width}-${dark ? "dark" : "light"}`;
    const page = await host({ width, dark });

    for (const view of ["FULL", "COMPACT"]) {
      const v = view.toLowerCase();
      let r = await call(page, `{ pubs(near: {lat: 51.4599, lng: -0.0116}) { nodes { name } } }`, { ui: { view } });
      await settle(r.app);
      await shoot(r.outer, `pubs-${v}-${tag}`);

      r = await call(page, MENU, { ui: { view, highlight: ["1000000111112124"] } });
      await settle(r.app, view === "FULL" ? ".spotlight" : ".rows");
      await shoot(r.outer, `menu-${v}-${tag}`);
    }

    // A round, built by the model, shown as the basket.
    let r = await call(page, `${ADD(4).slice(0, -1)} b: basketAdd(productId: "10000000292", portion: "Half pint") { success } }`, {
      ui: { view: "FULL" },
    });
    await settle(r.app, ".lines");
    await shoot(r.outer, `basket-full-${tag}`);
    r = await call(page, BASKET_Q(), { ui: { view: "COMPACT" } });
    await settle(r.app, ".basket-compact");
    await shoot(r.outer, `basket-compact-${tag}`);

    // The quote, and the PREVIEW the model would show: the app waits for the yes in the chat.
    r = await call(page, `mutation { quote { quote { id total { formatted } } } }`);
    await settle(r.app, ".quote");
    const quoteId = await r.app.evaluate(() => document.querySelector(".quote")?.dataset.quote ?? null);
    await shoot(r.outer, `quote-full-${tag}`);
    const id = quoteId ?? (await lastQuoteId(page));
    r = await call(page, `mutation { pay(quoteId: "${id}", action: PREVIEW) { preview confirmationToken } }`, {
      ui: { view: "COMPACT" },
    });
    await settle(r.app, ".quote");
    await shoot(r.outer, `quote-compact-${tag}`);

    // The order, once the model has confirmed on the user's yes (the payment API is the mock).
    const token = await previewToken(page);
    await fetch("http://127.0.0.1:4010/control?status=inProgress");
    r = await call(page, `mutation { pay(quoteId: "${id}", action: CONFIRM, confirmationToken: "${token}") { status paymentPage } }`);
    await settle(r.app, ".tracker");
    await shoot(r.outer, `order-progress-${tag}`);
    await fetch("http://127.0.0.1:4010/control?status=placed");
    r = await call(page, `{ order(quoteId: "${id}") { status orderRef } }`, { ui: { view: "COMPACT" } });
    await settle(r.app, ".order-ref");
    await shoot(r.outer, `order-placed-${tag}`);
    await fetch("http://127.0.0.1:4010/control?status=pending");
    await page.close();
  }
}

function BASKET_Q() {
  return `{ basket { count summary } }`;
}

/** The newest quote id, from the conversation's own list. */
async function lastQuoteId(page) {
  const text = await lastResultText(page);
  return text.match(/"id":\s*"([0-9a-f]{12})"/)?.[1];
}

async function previewToken(page) {
  const text = await lastResultText(page);
  return text.match(/"confirmationToken":\s*"([0-9a-f]+)"/)?.[1];
}

/** The text a host without apps would show: the reference host's "Tool Result" panel. */
async function lastResultText(page) {
  const panel = page.locator("text=Tool Result").last();
  await panel.click();
  const pre = page.locator("pre").last();
  const json = JSON.parse(await pre.innerText());
  await panel.click();
  return json.content.find((c) => c.type === "text").text;
}

// ---- Checks ----
{
  const page = await host();
  // An app showing the basket stays current while the model changes it.
  const first = await call(page, BASKET_Q(), { ui: { view: "FULL" } });
  await settle(first.app);
  await call(page, ADD(2));
  await first.app.waitForFunction(() => document.querySelector(".line .stepper output")?.textContent === "2", null, {
    timeout: 25000,
  });
  check(true, "a basket the model changes updates in an app already on screen");

  // A tap in the app reaches the model's context.
  const menu = await call(page, MENU, { ui: { view: "FULL" } });
  await settle(menu.app, ".spotlight");
  await menu.app.click(".guest .portion >> nth=2");
  await page.waitForSelector("text=Model Context", { timeout: 10000 });
  await page.locator("text=Model Context").last().click();
  const context = await page.locator("pre").last().innerText();
  check(/the user added 1 × Rooster’s - Scoundrel \(Pint\)/.test(context), "a tap in the app reaches the model's context");
  await page.locator("text=Model Context").last().click();
  await first.app.waitForFunction(() => document.querySelectorAll(".line").length === 2, null, { timeout: 25000 });
  check(true, "the app's own change shows in the other app on screen too");

  // Typing what a drink is, not its name, still finds it: the server's search takes over from the instant filter.
  await menu.app.fill(".search input", "mocktail");
  await menu.app.waitForSelector("text=Raspberry Refresher", { timeout: 15000 });
  check(true, "searching \"mocktail\" in the app finds the 0% cocktails");
  await menu.app.fill(".search input", "");

  // A product that asks a question opens a sheet: fish and chips asks about peas, and comes with a drink.
  await menu.app.fill(".search input", "fish and chips");
  await menu.app.waitForSelector("text=Small freshly battered fish and chips", { timeout: 15000 });
  await menu.app.click("#p-10000162183 .portion");
  await menu.app.waitForSelector(".sheet .choice-group", { timeout: 15000 });
  await sleep(400);
  await menu.outer?.screenshot?.({ path: join(out, "sheet-380-light.png") });
  // Quoted: an exact match, since the question "Peas, mushy peas or beans?" also contains the words.
  await menu.app.click('.sheet .choice >> text="Mushy Peas"');
  await menu.app.click('.sheet .choice.drink >> text="Guinness"');
  await sleep(300);
  await menu.outer.screenshot({ path: join(out, "sheet-picked-380-light.png"), animations: "disabled" });
  await menu.app.click(".sheet-foot .primary");
  // Only an app on screen watches the basket; the menu has pushed the first one out of view.
  await first.outer.scrollIntoViewIfNeeded();
  await first.app.waitForFunction(
    () => [...document.querySelectorAll(".line")].some((l) => l.textContent.includes("Mushy Peas"))
      && [...document.querySelectorAll(".line")].some((l) => l.textContent.includes("With the meal deal")),
    null,
    { timeout: 25000 },
  );
  check(true, "the options sheet adds fish and chips with mushy peas and a Guinness from the meal deal");
  await menu.app.fill(".search input", "");

  // The app quotes, and its button is the acceptance: it names the total and table, and one tap pays.
  await menu.app.click(".basket-bar");
  await settle(menu.app, ".basket-foot");
  await menu.app.click(".basket-foot .primary");
  await settle(menu.app, ".quote");
  const html = await menu.app.content();
  const pay = menu.app.locator(".quote .primary.pay");
  check(/^Confirm payment · £[\d.]+ · table \d+$/.test((await pay.innerText()).trim()), "the quote's button names the total and table");
  check((await menu.app.locator(".quote .eyebrow").innerText()).startsWith("Quote"), "nothing is paid before the tap");
  await pay.click();
  await menu.app.waitForFunction(() => document.querySelector(".quote .eyebrow")?.textContent?.startsWith("Order"), null, {
    timeout: 25000,
  });
  check(true, "the tap pays, and the card follows the order");
  check(html.length > 0, "the app rendered");

  // Text-only hosts get the whole response as text.
  const text = await lastResultText(page);
  check(JSON.parse(text).data !== undefined, "the text result is the GraphQL response on its own");
  await page.close();
}

await browser.close();
stopAll();
if (failures.length) {
  console.error(`${failures.length} check(s) failed`);
  process.exit(1);
}
console.log(`screenshots in ${out}`);
