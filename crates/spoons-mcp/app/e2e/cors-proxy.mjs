// The reference host connects to MCP servers from the browser, so it needs CORS, which a real host (connecting
// server to server) doesn't and the server doesn't offer. This forwards to it with the headers added, and, as
// mcp-gateway does, the receipt address as X-Spoons-Email. Tests only.
//
//   node e2e/cors-proxy.mjs 3002 http://127.0.0.1:3001 you@example.com

import { createServer, request } from "node:http";

const [port = "3002", target = "http://127.0.0.1:3001", email] = process.argv.slice(2);
const upstream = new URL(target);
const cors = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, DELETE, OPTIONS",
  "access-control-allow-headers": "*",
  "access-control-expose-headers": "mcp-session-id, mcp-protocol-version",
};

createServer((req, res) => {
  if (req.method === "OPTIONS") {
    res.writeHead(204, cors);
    return res.end();
  }
  const out = request(
    { host: upstream.hostname, port: upstream.port, path: req.url, method: req.method, headers: { ...req.headers, host: upstream.host, ...(email ? { "x-spoons-email": email } : {}) } },
    (up) => {
      res.writeHead(up.statusCode ?? 502, { ...up.headers, ...cors });
      up.pipe(res);
    },
  );
  out.on("error", (e) => {
    res.writeHead(502, cors);
    res.end(String(e));
  });
  req.pipe(out);
}).listen(Number(port), "127.0.0.1", () => console.log(`CORS proxy on http://127.0.0.1:${port} -> ${target}`));
