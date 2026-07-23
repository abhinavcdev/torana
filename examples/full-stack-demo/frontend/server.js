// Minimal static file server for the frontend half of the demo. Zero npm
// dependencies -- in a real app this would be your framework's build output
// served by nginx/Caddy/whatever; the point here is that it's a *separate*
// process from the API, so torana is doing genuine multi-upstream routing.
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const PORT = process.env.PORT || 8082;
const ROOT = __dirname;

const TYPES = { ".html": "text/html", ".css": "text/css", ".js": "application/javascript" };

const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  let filePath = url.pathname === "/" ? "/index.html" : url.pathname;
  filePath = path.normalize(filePath).replace(/^(\.\.[/\\])+/, "");
  const full = path.join(ROOT, filePath);

  fs.readFile(full, (err, data) => {
    if (err) {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end("Not found");
      return;
    }
    const ext = path.extname(full);
    res.writeHead(200, { "Content-Type": TYPES[ext] || "application/octet-stream" });
    res.end(data);
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`frontend listening on 127.0.0.1:${PORT}`);
});
