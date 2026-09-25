#!/usr/bin/env node
// Zero-dependency static server for browser mock mode and UI screenshots (DEVELOPMENT.md §2, §6).
//
//   node scripts/serve-ui.mjs [--root <dir>] [--port <n>]
//
// Serves <root>/ui at / and <root>/ui-dev at /dev/ on 127.0.0.1 only, with the app's CSP (read from
// <root>/src-tauri/tauri.conf.json) as the Content-Security-Policy header on every response.
// --root defaults to the repository root; --port defaults to 5173. --port 0 picks a free port.
// The first stdout line is always "listening on <port>" (scripts parse it); nothing else goes to stdout.

import { createReadStream, readFileSync } from "node:fs";
import { realpath, stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HOST = "127.0.0.1";
const USAGE = "usage: node scripts/serve-ui.mjs [--root <dir>] [--port <n>]";

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8",
};

function fail(message, code = 2) {
  process.stderr.write(`serve-ui: ${message}\n`);
  process.exit(code);
}

function parseArgs(argv) {
  const opts = {
    root: path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."),
    port: 5173,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      process.stdout.write(`${USAGE}\n`);
      process.exit(0);
    }
    if (arg !== "--root" && arg !== "--port") fail(`unknown argument: ${arg}\n${USAGE}`);
    const value = argv[++i];
    if (value === undefined) fail(`${arg} needs a value\n${USAGE}`);
    if (arg === "--root") {
      opts.root = path.resolve(value);
    } else {
      if (!/^\d{1,5}$/.test(value) || Number(value) > 65535) fail(`invalid port: ${value}`);
      opts.port = Number(value);
    }
  }
  return opts;
}

function readCsp(root) {
  const confPath = path.join(root, "src-tauri", "tauri.conf.json");
  let csp;
  try {
    csp = JSON.parse(readFileSync(confPath, "utf8"))?.app?.security?.csp;
  } catch (err) {
    fail(`cannot read the CSP from ${confPath}: ${err.message}`, 1);
  }
  if (typeof csp !== "string" || csp.trim() === "") {
    fail(`${confPath} has no app.security.csp string`, 1);
  }
  return csp;
}

// Maps a URL path to a file inside one of the served directories, or null.
// Rejects anything that resolves outside the directory, including through symlinks.
async function resolveFile(mounts, urlPath) {
  let decoded;
  try {
    decoded = decodeURIComponent(urlPath);
  } catch {
    return null;
  }
  if (decoded.includes("\0") || decoded.includes("\\")) return null;

  const mount = mounts.find((m) => decoded === m.prefix.slice(0, -1) || decoded.startsWith(m.prefix));
  if (!mount) return null;
  let rel = decoded === mount.prefix.slice(0, -1) ? "" : decoded.slice(mount.prefix.length);
  if (rel === "" || rel.endsWith("/")) rel += "index.html";

  const candidate = path.resolve(mount.dir, ...rel.split("/"));
  if (!isInside(mount.dir, candidate)) return null;
  try {
    const real = await realpath(candidate);
    if (!isInside(mount.realDir, real)) return null;
    const info = await stat(real);
    return info.isFile() ? real : null;
  } catch {
    return null;
  }
}

// True if p is strictly inside dir (both absolute).
function isInside(dir, p) {
  const rel = path.relative(dir, p);
  return rel !== "" && rel !== ".." && !rel.startsWith(`..${path.sep}`) && !path.isAbsolute(rel);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const csp = readCsp(opts.root);

  // Longest prefix first, so /dev/ wins over /.
  const mounts = [];
  for (const [prefix, sub] of [["/dev/", "ui-dev"], ["/", "ui"]]) {
    const dir = path.join(opts.root, sub);
    let realDir;
    try {
      realDir = await realpath(dir);
    } catch {
      fail(`missing directory: ${dir}`, 1);
    }
    mounts.push({ prefix, dir, realDir });
  }

  const server = createServer(async (req, res) => {
    const headers = {
      "Content-Security-Policy": csp,
      "X-Content-Type-Options": "nosniff",
      "Cache-Control": "no-store",
    };
    const send = (status, body) => {
      res.writeHead(status, { ...headers, "Content-Type": "text/plain; charset=utf-8" });
      res.end(req.method === "HEAD" ? undefined : body);
    };
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.setHeader("Allow", "GET, HEAD");
      send(405, "method not allowed\n");
      return;
    }
    let urlPath;
    try {
      urlPath = new URL(req.url ?? "/", `http://${HOST}`).pathname;
    } catch {
      send(400, "bad request\n");
      return;
    }
    const file = await resolveFile(mounts, urlPath);
    if (!file) {
      send(404, "not found\n");
      return;
    }
    const type = CONTENT_TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream";
    res.writeHead(200, { ...headers, "Content-Type": type });
    if (req.method === "HEAD") {
      res.end();
      return;
    }
    const stream = createReadStream(file);
    stream.on("error", () => res.destroy());
    stream.pipe(res);
  });

  server.on("error", (err) => fail(`cannot listen on ${HOST}:${opts.port}: ${err.message}`, 1));
  server.listen(opts.port, HOST, () => {
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : opts.port;
    process.stdout.write(`listening on ${port}\n`);
  });
}

main();
