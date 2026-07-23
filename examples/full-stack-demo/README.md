# Full-stack demo: torana in front of a real app

A minimal but genuinely working full-stack app — a static frontend and a
separate JSON API, each its own process with its own port — proxied through
one torana listener. The point isn't the app (a visit counter); it's proving
torana's routing, health checks, and automatic HTTPS against a real deploy,
not a synthetic benchmark.

```
                         ┌─────────────┐
  browser ── :443 ──────▶│   torana    │
  (HTTPS, ACME cert)     │             │
                         │ /api/*  ────┼──▶ api backend      127.0.0.1:8081
                         │ /*      ────┼──▶ frontend server  127.0.0.1:8082
                         └─────────────┘
```

## 1. Run it locally first (no server, no domain, no cost)

Proves the routing and both backends work before you touch a real machine.

```bash
# from the repo root
cargo build --release -p torana

cd examples/full-stack-demo
node api/server.js &
node frontend/server.js &
../../target/release/torana --config deploy/torana.local.toml &

curl http://127.0.0.1:8080/                 # the frontend, through torana
curl -X POST http://127.0.0.1:8080/api/visits   # the API, through torana
curl http://127.0.0.1:9090/healthz          # torana's own health endpoint
```

Open `http://127.0.0.1:8080/` in a browser — the counter should increment
on every reload. Kill all three (`pkill -f full-stack-demo`, `pkill torana`)
when done.

## 2. The cheapest real setup

**A ~$4/mo VPS** ([Hetzner](https://www.hetzner.com/cloud/) CX22 or the ARM
CAX11, DigitalOcean/Vultr's cheapest droplet — any of these work). This is
the recommended option: cheap enough that cost is a non-issue, and it's a
genuine public IP + real domain + real Let's Encrypt certificate, which is
what "production" actually looks like. Two free alternatives, with real
tradeoffs:

- **Oracle Cloud's Always Free tier** (4 ARM cores, 24GB RAM, genuinely
  $0/mo forever) — the free ARM shapes are frequently out of capacity in
  popular regions, so provisioning can take repeated tries.
- **Hardware you already own** (a Raspberry Pi, an old laptop) — $0
  marginal cost, but you're either port-forwarding on your home router
  (exposes your home IP) or fronting it with a tunnel (Cloudflare Tunnel,
  Tailscale Funnel), which changes who terminates TLS. Skip to
  [the tunnel note](#home-hardware-instead-of-a-vps) if that's your route.

The steps below assume a fresh Ubuntu 22.04/24.04 VPS and a domain (or
subdomain) you can point an A record at. Total time: under 15 minutes.

## 3. Point DNS at the box

Create an A record (and AAAA if the VPS has IPv6) for the subdomain you'll
use, e.g. `demo.yourdomain.com` → the VPS's public IP. Give it a few minutes
to propagate (`dig demo.yourdomain.com` should return the IP).

## 4. Firewall: only 443 needs to be open

torana's ACME uses the TLS-ALPN-01 challenge, which validates over the same
port your app already serves on — port 80 is never needed.

```bash
sudo ufw allow 22/tcp    # don't lock yourself out of SSH
sudo ufw allow 443/tcp
sudo ufw enable
```

## 5. Install Node (for the demo app) and build torana

```bash
# Node, for the demo's two backend processes
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt-get install -y nodejs

# torana itself: either grab a prebuilt release binary...
curl -L https://github.com/abhinavcdev/torana/releases/latest/download/torana-latest-x86_64-unknown-linux-musl.tar.gz | tar xz
sudo mv torana /usr/local/bin/torana
# ...or build from source with the acme feature (needed for automatic HTTPS):
#   git clone https://github.com/abhinavcdev/torana && cd torana
#   cargo build --release -p torana --features acme
#   sudo cp target/release/torana /usr/local/bin/torana
```

If you used the prebuilt binary, confirm it actually has ACME support before
relying on it — the default release build does **not** enable the `acme`
feature (it's opt-in to keep the default binary small), so for this demo
you need the from-source build with `--features acme`, or check the
release notes for an acme-enabled artifact.

## 6. Lay out the files

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin torana

sudo mkdir -p /opt/torana-demo /etc/torana /var/lib/torana/acme-cache
sudo cp -r api frontend /opt/torana-demo/
sudo cp deploy/torana.prod.toml /etc/torana/torana.toml
# edit the domain + email in it:
sudo nano /etc/torana/torana.toml

sudo chown -R torana:torana /opt/torana-demo /var/lib/torana
sudo chown torana:torana /etc/torana/torana.toml
```

Uncomment `staging = true` in `torana.toml` for your first attempt — Let's
Encrypt's real directory has strict rate limits, and staging certs (which
your browser won't trust, but which prove the whole pipeline works) don't.
Once a `torana ready` log line shows up with no ACME errors, comment
`staging` back out, restart, and get the real, browser-trusted certificate.

## 7. Install and start the services

```bash
sudo cp deploy/torana.service deploy/api.service deploy/frontend.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now api frontend torana
sudo systemctl status torana   # should show "active (running)"
sudo journalctl -u torana -f   # watch the ACME order happen live
```

## 8. Verify it's real

```bash
curl -I https://demo.yourdomain.com/                  # real HTTPS, real cert
curl -X POST https://demo.yourdomain.com/api/visits    # real API call
```

Then, from your own machine (not the server): check
[SSL Labs](https://www.ssllabs.com/ssltest/) against your domain for a
real TLS grade, and open the URL in an actual browser — the padlock should
show a trusted certificate, not a warning.

## 9. Prove the reload story works too

```bash
# on the VPS, edit torana.toml (e.g. bump the health check interval), then:
sudo systemctl reload torana
sudo journalctl -u torana -n 5   # "Config reloaded successfully", no dropped connections
```

## Home hardware instead of a VPS

If you're using a Raspberry Pi or similar behind a home router: use
[Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/)
or Tailscale Funnel to get a public HTTPS endpoint without opening a port on
your router. In that setup the tunnel terminates TLS at Cloudflare's edge
and forwards plain HTTP to torana on your Pi — so use `torana.local.toml`'s
plain-HTTP listener (on whatever port the tunnel forwards to) instead of
`torana.prod.toml`'s ACME config; torana's own ACME feature is for when
*it* owns the public IP and certificate, which isn't the case behind a
tunnel.

## Cost and cleanup

A Hetzner CX22 is billed hourly (~€0.006/hr) — spin it up, run through this
whole guide, and destroy it when you're done testing, and the entire
exercise costs a few cents. `hcloud server delete` (or the equivalent in
the DO/Vultr/whatever console) tears it down completely; there's nothing
else billed once the server is gone.
