// Wetherspoon's two APIs, played from the fixtures the Rust tests use, so the app can be driven end to end without
// the live API and never near the real payment endpoint. `spoons mcp` reaches it through SPOONS_API_BASE and
// SPOONS_PAYMENTS_BASE (with SPOONS_API_KEY set to anything).
//
//   node e2e/fixture-api.mjs 4010
//   GET /control?status=pending|inProgress|paid|placed|duplicate   moves the mock payment along

import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";

const port = Number(process.argv[2] ?? 4010);
const fixtures = join(dirname(fileURLToPath(import.meta.url)), "../../tests/fixtures");
const read = (name) => readFileSync(join(fixtures, name));
const TX = "025420261120E2E";

const api = "/api/v0.1/jdw/venues/254";
const routes = {
  "/api/v0.1/venues": "venues.json",
  [api]: "venue.json",
  [`${api}/ales`]: "ales.json",
  [`${api}/wait-times`]: "wait-times.json",
  [`${api}/tables`]: "tables.json",
  [`${api}/sales-areas/443/menus`]: "menus-443.json",
  [`${api}/sales-areas/444/menus`]: "menus-444.json",
};
for (const id of ["34585", "13385", "24203", "34586"]) routes[`${api}/sales-areas/443/menus/${id}`] = `menu-443-${id}.json`;
routes[`${api}/sales-areas/444/menus/34585`] = "menu-444-34585.json";

let payment = { order: "pending", payment: "pending" };
const states = {
  pending: { order: "pending", payment: "pending" },
  inProgress: { order: "pending", payment: "inProgress" },
  paid: { order: "paid", payment: "paid" },
  duplicate: { order: "duplicate", payment: "paid" },
  placed: "placed",
};

const send = (res, status, body) => {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
};

createServer((req, res) => {
  const url = new URL(req.url, "http://x");
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const path = url.pathname;
    if (path === "/control") {
      payment = states[url.searchParams.get("status")] ?? payment;
      return send(res, 200, { ok: true });
    }
    if (req.method === "GET" && routes[path]) return send(res, 200, read(routes[path]));
    if (req.method === "POST" && path.endsWith("/basket/validate")) {
      // The till charges a penny more a line than the menu, so a quote shows the difference.
      const items = JSON.parse(body).items.map((line) => ({
        id: line.product.id,
        quantity: line.quantity,
        price: { value: Math.round(line.quantity * 370) / 100 },
      }));
      const total = Math.round(items.reduce((t, i) => t + i.price.value, 0) * 100) / 100;
      return send(res, 200, { data: { id: "b1", total: { value: total, currency: "GBP" }, items, token: "t" } });
    }
    if (req.method === "POST" && path === "/pay/jdw/venues/254/orders/prepare/card") {
      return send(res, 200, {
        success: true,
        data: { transactionId: TX, url: "https://paymentpage.axepta.bnpparibas/payssl.aspx?e2e=1", duplicateOrder: false },
      });
    }
    if (req.method === "GET" && path === `/pay/jdw/venues/254/payment/${TX}/status`) {
      if (payment === "placed") {
        return send(res, 200, { success: true, data: { uniqueRef: 4821, paidBy: "card", placedAt: new Date().toISOString() } });
      }
      return send(res, 400, { success: false, data: { orderStatus: payment.order, paymentStatus: payment.payment, transactionId: TX } });
    }
    send(res, 404, { error: `no fixture for ${req.method} ${path}` });
  });
}).listen(port, "127.0.0.1", () => console.log(`fixture API on http://127.0.0.1:${port}`));
