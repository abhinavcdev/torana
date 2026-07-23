// Minimal API backend for the torana full-stack demo. Zero npm dependencies
// on purpose -- `node server.js` is the entire deploy story on the VPS side.
// State is a single JSON file, which is all a demo like this needs; swap in
// a real database and this file barely changes shape.
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const PORT = process.env.PORT || 8081;
const DATA_FILE = path.join(__dirname, "data.json");

function readState() {
  try {
    return JSON.parse(fs.readFileSync(DATA_FILE, "utf8"));
  } catch {
    return { count: 0 };
  }
}

function writeState(state) {
  fs.writeFileSync(DATA_FILE, JSON.stringify(state));
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  res.setHeader("Content-Type", "application/json");

  if (url.pathname === "/api/health") {
    res.writeHead(200);
    res.end(JSON.stringify({ status: "ok" }));
    return;
  }

  if (url.pathname === "/api/visits" && req.method === "POST") {
    const state = readState();
    state.count += 1;
    state.last_visit = new Date().toISOString();
    writeState(state);
    res.writeHead(200);
    res.end(JSON.stringify(state));
    return;
  }

  if (url.pathname === "/api/visits" && req.method === "GET") {
    res.writeHead(200);
    res.end(JSON.stringify(readState()));
    return;
  }

  res.writeHead(404);
  res.end(JSON.stringify({ error: "not found" }));
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`api listening on 127.0.0.1:${PORT}`);
});
