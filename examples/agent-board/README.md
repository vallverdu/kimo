# Agent board: an example app on top of kimo

A separate PWA (its own origin, its own look) that watches and drives kimo's agents with an
app key: live list with memory, CPU and "quiet for", pause / stop / continue, an embedded
`<kimo-terminal>` and a message box. It is one static HTML file: everything comes from kimo's SDK.

```bash
# 1. kimo running (e.g. cargo run --release), then create a key for this app's origin:
curl -s -X POST http://127.0.0.1:7777/api/clients -H 'content-type: application/json' \
  -d '{"name":"Agent board","origins":["http://127.0.0.1:8080"],"scope":"control"}'
# (or kimo → Apps → +)

# 2. serve this folder from another origin and open it
python3 -m http.server 8080 --bind 127.0.0.1
# http://127.0.0.1:8080/?kimo=http://127.0.0.1:7777&key=kimo_…
```

On the tailnet, serve both over HTTPS (`tailscale serve`) and use the HTTPS origins: a page on
HTTPS cannot call kimo over plain HTTP.
