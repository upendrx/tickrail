# Deployment

## Where to run

Latency to the venue dominates everything else. For a crypto exchange hosted
in a public cloud, run in the same cloud region. Most large exchanges publish
theirs, or it can be found by measuring round-trip time. For traditional
exchanges, it means colocation in the exchange's data centre.

## systemd

```ini
# /etc/systemd/system/tickrail.service
[Unit]
Description=tickrail
After=network-online.target
Wants=network-online.target

[Service]
User=tickrail
WorkingDirectory=/opt/tickrail
ExecStart=/opt/tickrail/tickrail run -c /opt/tickrail/config/live.toml
Restart=on-failure
RestartSec=5
# Credentials for adapters that need them (e.g. Alpaca):
EnvironmentFile=-/etc/tickrail/env
# Optional: pin the process to isolated cores
# CPUAffinity=2-5

[Install]
WantedBy=multi-user.target
```

Journals grow at tens of MB per hour per symbol. Rotate them by putting a date
in the path from your launcher, and ship them to object storage for research.

## Docker

```bash
docker build -t tickrail .
docker run -d --name tickrail --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v /srv/tickrail/data:/app/data \
  tickrail run -c config/binance.toml --set http.listen=0.0.0.0:8080
```

For latency-sensitive runs, use `--network host` and `--cpuset-cpus`.

## Securing the console

The console and API have no authentication and can trip the kill switch.

- Keep `http.listen` on `127.0.0.1` and reach it with `ssh -L 8080:localhost:8080 host`.
- Or put it behind a reverse proxy that authenticates (nginx with basic auth,
  oauth2-proxy, Cloudflare Access).
- Or set `http.enabled = false` on production boxes and read snapshots another way.

## Credentials

Adapters that need keys read them from the environment first (for example
`APCA_API_KEY_ID`). Keep keys out of config files you commit.
