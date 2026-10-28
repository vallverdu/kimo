//! The script the coordinator runs on a new machine over SSH (`sh -s`, on standard input):
//! install kimo when it is missing, run it as a service bound to the fleet's network, wait for
//! it, and print a pairing code made there (requests from the machine itself are its owner).
//! It prints `kimo-step: …` as it goes, `kimo-note: …` for what you may want to fix, and
//! `KIMO_PAIRING_CODE=…` at the end.

/// What to set up.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup<'a> {
    /// Install kimo (or a newer one) even if one is there.
    pub install: bool,
    /// The release to install (`v0.1.15`); `None` for the latest.
    pub version: Option<&'a str>,
    /// Where its kimo listens: `tailscale`, `zerotier` or an address.
    pub bind: &'a str,
    /// Its name for the other kimos.
    pub name: &'a str,
    pub port: u16,
    /// Where install.sh comes from.
    pub install_url: &'a str,
}

pub const INSTALL_URL: &str = "https://raw.githubusercontent.com/vallverdu/kimo/main/install.sh";

/// Single-quoted for sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub fn script(s: &Setup) -> String {
    let port = s.port;
    let version = s
        .version
        .map(|v| format!("KIMO_VERSION={} ", quote(v)))
        .unwrap_or_default();
    let env = format!(
        "KIMO_BIND={}\nKIMO_NAME={}\nKIMO_PORT={port}\n",
        s.bind, s.name
    );
    format!(
        r#"set -eu
PATH="$HOME/.local/bin:$PATH"; export PATH
os="$(uname -s)"
command -v curl >/dev/null 2>&1 || {{ echo "kimo-error: curl is missing on this machine: install it first"; exit 1; }}
status() {{ curl -fs -o /dev/null -H "Host: localhost:{port}" "http://127.0.0.1:{port}/api/status"; }}

if {install} || ! command -v kimo >/dev/null 2>&1; then
  echo "kimo-step: installing kimo"
  extra=""; [ "$os" = Linux ] && extra="--server"
  curl -fsSL {url} | {version}sh -s -- $extra
fi

if ! status; then
  echo "kimo-step: starting kimo as a service"
  if [ "$os" = Linux ]; then
    dir="$HOME/.config/systemd/user"
    mkdir -p "$dir/kimo.service.d"
    if [ ! -f "$dir/kimo.service" ]; then
      cat > "$dir/kimo.service" <<'UNIT'
[Unit]
Description=kimo agent environment
After=network-online.target tailscaled.service zerotier-one.service
Wants=network-online.target

[Service]
ExecStart=%h/.local/bin/kimo serve
Restart=on-failure
Environment=PATH=%h/.local/bin:%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin
EnvironmentFile=-%h/.kimo/env

[Install]
WantedBy=default.target
UNIT
    fi
    printf '[Service]\n{drop_in}' > "$dir/kimo.service.d/fleet.conf"
    systemctl --user daemon-reload
    systemctl --user enable kimo >/dev/null 2>&1 || true
    systemctl --user restart kimo
    me="$(id -un)"
    loginctl enable-linger "$me" 2>/dev/null || sudo -n loginctl enable-linger "$me" 2>/dev/null \
      || echo "kimo-note: kimo stops when $me logs out: run 'sudo loginctl enable-linger $me' there once"
  else
    plist="$HOME/Library/LaunchAgents/ai.kimo.plist"
    mkdir -p "$HOME/Library/LaunchAgents"
    cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>ai.kimo</string>
  <key>ProgramArguments</key><array><string>$HOME/.local/bin/kimo</string><string>serve</string></array>
  <key>EnvironmentVariables</key><dict>
{plist_env}    <key>PATH</key><string>$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict></plist>
PLIST
    launchctl bootout "gui/$(id -u)/ai.kimo" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$plist" 2>/dev/null || launchctl load -w "$plist" \
      || echo "kimo-note: log in to this Mac once so kimo can run in your session"
  fi
fi

echo "kimo-step: waiting for kimo"
i=0
until status; do
  i=$((i + 1))
  [ "$i" -gt 60 ] && {{ echo "kimo-error: kimo did not start: see journalctl --user -u kimo (Linux) or Console (macOS)"; exit 1; }}
  sleep 1
done

echo "kimo-step: making a pairing code"
code="$(curl -fsS -X POST -H "Host: localhost:{port}" "http://127.0.0.1:{port}/api/pairing" | sed -n 's/.*"code" *: *"\([^"]*\)".*/\1/p')"
[ -n "$code" ] || {{ echo "kimo-error: kimo gave no pairing code"; exit 1; }}
echo "KIMO_PAIRING_CODE=$code"
"#,
        install = if s.install { "true" } else { "false" },
        url = quote(s.install_url),
        drop_in = env
            .lines()
            .map(|l| format!("Environment={l}\\n"))
            .collect::<String>(),
        plist_env = env
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| format!("    <key>{k}</key><string>{v}</string>\n"))
            .collect::<String>(),
    )
}

/// What the script printed: its steps, notes, error and the pairing code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Said {
    pub steps: Vec<String>,
    pub notes: Vec<String>,
    pub error: Option<String>,
    pub code: Option<String>,
}

pub fn read_output(out: &str) -> Said {
    let mut said = Said::default();
    for line in out.lines().map(str::trim) {
        if let Some(s) = line.strip_prefix("kimo-step: ") {
            said.steps.push(s.to_string());
        } else if let Some(n) = line.strip_prefix("kimo-note: ") {
            said.notes.push(n.to_string());
        } else if let Some(e) = line.strip_prefix("kimo-error: ") {
            said.error = Some(e.to_string());
        } else if let Some(c) = line.strip_prefix("KIMO_PAIRING_CODE=") {
            said.code = Some(c.to_string());
        }
    }
    said
}

/// Is the name or the bind safe to put in the script (letters, digits, `.-_:`)?
pub fn safe_word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Setup<'static> {
        Setup {
            install: false,
            version: Some("v0.1.15"),
            bind: "tailscale",
            name: "nuc-extreme",
            port: 7777,
            install_url: INSTALL_URL,
        }
    }

    #[test]
    fn the_script_installs_runs_and_pairs() {
        let s = script(&setup());
        assert!(s.contains("KIMO_VERSION='v0.1.15' sh -s -- $extra"));
        assert!(s.contains("if false || ! command -v kimo"));
        assert!(s.contains(r"Environment=KIMO_BIND=tailscale\nEnvironment=KIMO_NAME=nuc-extreme\nEnvironment=KIMO_PORT=7777\n"));
        assert!(s.contains("<key>KIMO_BIND</key><string>tailscale</string>"));
        assert!(s.contains("/api/pairing"));
        assert!(script(&Setup {
            install: true,
            version: None,
            ..setup()
        })
        .contains(
            "curl -fsSL 'https://raw.githubusercontent.com/vallverdu/kimo/main/install.sh' | sh -s"
        ));
    }

    #[test]
    fn the_script_is_valid_sh() {
        let ok = std::process::Command::new("sh")
            .args(["-n", "-c", &script(&setup())])
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    #[test]
    fn its_output() {
        let said = read_output(
            "kimo-step: installing kimo\nnoise\nkimo-note: linger\nKIMO_PAIRING_CODE=K7M2-9QXP\n",
        );
        assert_eq!(said.steps, ["installing kimo"]);
        assert_eq!(said.notes, ["linger"]);
        assert_eq!(said.code.as_deref(), Some("K7M2-9QXP"));
        assert!(read_output("kimo-error: curl is missing").error.is_some());
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert!(
            safe_word("nuc-extreme")
                && safe_word("10.147.17.21")
                && !safe_word("a b")
                && !safe_word("$(x)")
        );
    }
}
