# Running the repository on a server

The default setup serves packages from the machine you build on, at
`http://10.0.2.2:8080` — the QEMU host as the VM sees it. That is right for
development and wrong for anything else. This directory is what you need to put
the repository on a server you rent and point appliances at it.

## The shape of it

```
build machine                          rented server            appliance
─────────────                          ─────────────            ─────────
mkrepo.sh                              zrpkg-server             zrpkg
  signs with repo.key   ──publish──▶     serves .zrp/.json  ◀──fetches──
  keeps the key                          holds no secrets       verifies against
                                                                trusted.key
```

Two things are worth being precise about:

**The repository is public data.** Every archive is signed, and every client
verifies it against `/etc/vakt/trusted.key` in its own image. A server that
hands out the wrong bytes produces failed signature checks, not compromised
appliances. So there is no authentication on the server, and nothing on it is
worth stealing.

**That is only true while the signing key stays put.** `build-system/keys/repo.key`
must never reach the server. Signing happens on the build machine; what travels
is the signed output. `publish.sh` refuses to copy anything matching `*.key` for
that reason.

## Setting it up

**1. Install the server.** Build `zrpkg-server` (it is a static Go binary, so
build it anywhere and copy it) and put it on the host:

```bash
cd tools && go build -o bin/zrpkg-server ./cmd/zrpkg-server/
scp bin/zrpkg-server user@vps.example.com:/tmp/
```

```bash
# on the server
sudo install -m 0755 /tmp/zrpkg-server /usr/local/bin/
sudo useradd --system --home-dir /var/lib/zrpkg --create-home zrpkg
sudo install -m 0644 zrpkg-server.service /etc/systemd/system/
sudo systemctl enable --now zrpkg-server
```

The unit binds `127.0.0.1:8080` and expects a reverse proxy in front. To serve
the internet directly, change `-addr` to `:8080` and open the port — but read
the TLS section first.

**2. Publish.**

```bash
./deploy/publish.sh user@vps.example.com
```

This runs `mkrepo.sh`, then rsyncs the signed output. The index is copied last,
so a client never sees it advertise an archive that has not landed yet.

**3. Point the appliance at it.** From the panel's **Packages** page, or a
shell:

```bash
zrpkg repo https://packages.example.com
zrpkg update
```

The setting is written to `/persistent/etc/zrpkg.conf` on the data disk, so it
survives a reboot and a rebuilt image. `zrpkg repo` with no argument shows the
current value and which file it came from.

To ship images that already know, build with:

```bash
sudo VAKT_REPO_URL=https://packages.example.com ./build.sh
```

which bakes `/etc/vakt/zrpkg.conf` into the image. Anything written to the data
disk still overrides it.

## TLS

Signatures already give you integrity: plain HTTP cannot get a bad package
installed. What HTTP does leak is *which* packages an appliance fetches, to
anyone on the path. On a rented server that is worth closing.

**Behind a reverse proxy** (usual, and what the unit assumes) — the proxy holds
the certificate and talks to `127.0.0.1:8080`:

```nginx
server {
    listen 443 ssl;
    server_name packages.example.com;

    ssl_certificate     /etc/letsencrypt/live/packages.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/packages.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8080;
        # .zrp archives are large; do not buffer the whole thing first.
        proxy_buffering off;
    }
}
```

**Directly**, if you would rather not run a proxy:

```
ExecStart=/usr/local/bin/zrpkg-server -dir /var/lib/zrpkg/repo -addr :443 \
    -tls-cert /etc/letsencrypt/live/packages.example.com/fullchain.pem \
    -tls-key  /etc/letsencrypt/live/packages.example.com/privkey.pem
```

Binding 443 as a non-root user needs
`AmbientCapabilities=CAP_NET_BIND_SERVICE` in the unit, and the `zrpkg` user
needs read access to the certificate. Uncomment the `ReadOnlyPaths` line for
`/etc/letsencrypt` while you are there.

Appliance images carry the host's CA bundle at
`/etc/ssl/certs/ca-certificates.crt`, so an ordinary Let's Encrypt certificate
validates with no extra configuration.

## What the server will and will not serve

Serving a directory from a public address is a different problem from serving
it to a VM on your laptop, so `zrpkg-server` is deliberately narrow:

- `GET` and `HEAD` only; everything else is `405`.
- A flat namespace. Any request with a directory component is `404`, which
  leaves traversal with nowhere to resolve to.
- Only `.zrp` and `.json`. A stray key, an editor backup or a note in the
  repository directory is not reachable.
- No directory listings — the repository does not enumerate itself.
- Bounded read, write, header and idle timeouts.
- Per-IP rate limiting (`-rate-limit`, default 5 req/s, burst `-rate-burst`,
  default 20). A client past its budget gets `429` before any file is touched.
  Set `-rate-limit 0` to disable.
- `SIGTERM` drains in-flight downloads before exiting.

## Rotating the signing key

The key lives in `build-system/keys/repo.key` and is gitignored, so a fresh
clone generates a new one — and images built from it will reject packages signed
by the old key. If you regenerate deliberately:

1. `rm build-system/keys/repo.key` and run `./build-system/mkrepo.sh`.
2. Rebuild images so they pick up the new `/etc/vakt/trusted.key`.
3. Republish.

There is no in-place key rollover. Appliances trust exactly one key, the one
their image was built with, and changing it means reimaging.
