# Security

kimo runs terminals on your machines, so access to kimo is access to those machines.

## Model

- **Owner:** requests from the machine itself (not through a proxy; turn off with
  `KIMO_LOCAL_OWNER=false`), paired devices and paired kimos (each with its own revocable key),
  and the optional `KIMO_TOKEN`. The owner can do everything.
- **Pairing:** one-time codes, valid for 10 minutes, shown as a QR code or by `kimo pair`.
- **Apps:** keys limited to listed origins and to `read` or `control`; they cannot manage
  logins, keys, devices, paired kimos, skills or settings.
- **Network:** kimo listens on every interface by default (`KIMO_BIND`). Anything that is not
  the machine itself needs a key. Prefer private networks (Tailscale, ZeroTier, LAN) and do
  not expose kimo to the internet.
- **Secrets:** account credentials stay in each account's config folder under
  `~/.kimo/accounts`; keys are stored in `~/.kimo/state.json` with mode 600. Sessions do not
  inherit kimo's own environment secrets.
- **Agents:** agents run with your user's permissions. "Skip every prompt" (yolo) lets them
  run commands without asking; use it only where that is acceptable.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's "Report a vulnerability" (Security
tab of the repository) rather than in a public issue. Include the version (`kimo --version`),
the platform and the steps to reproduce.
