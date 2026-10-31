//! The SSH mesh: who should reach whom without a password, the check each machine runs, and
//! the shortcuts file every machine shares. Pure; the SSH is in `services::mesh`.

use super::registry::{Device, Registry};
use serde::Serialize;

/// A machine of the mesh: it has an account to log in to and an address.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub id: String,
    pub alias: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zerotier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tailscale: Option<String>,
    pub restricted: bool,
}

impl Node {
    /// Where to log in to it, preferring ZeroTier: SSH there goes to its OpenSSH, while on
    /// Tailscale it may be Tailscale SSH asking for a browser check.
    pub fn target(&self) -> String {
        let host = self.zerotier.as_ref().or(self.tailscale.as_ref());
        format!("{}@{}", self.user, host.map_or("", String::as_str))
    }
}

pub fn alias_of(id: &str, d: &Device) -> String {
    d.alias.clone().unwrap_or_else(|| format!("z{id}"))
}

/// The machines with an account and an overlay address, in id order.
pub fn nodes(r: &Registry) -> Vec<Node> {
    r.devices
        .iter()
        .filter_map(|(id, d)| {
            let user = d.user.clone()?;
            if d.zerotier.is_none() && d.tailscale.is_none() {
                return None;
            }
            Some(Node {
                id: id.clone(),
                alias: alias_of(id, d),
                user,
                zerotier: d.zerotier.clone(),
                tailscale: d.tailscale.clone(),
                restricted: r.mesh.restricted.contains(id),
            })
        })
        .collect()
}

/// What should happen when `from` logs in to `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    /// Log in without a password.
    Key,
    /// A restricted machine must need a password to reach the others.
    Password,
    /// Between two restricted machines: their own business.
    Either,
}

pub fn expected(from: &Node, to: &Node) -> Expect {
    match (from.restricted, to.restricted) {
        (false, _) => Expect::Key,
        (true, false) => Expect::Password,
        (true, true) => Expect::Either,
    }
}

/// What one login attempt found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reached {
    /// Logged in.
    Ok,
    /// SSH answers, the key is not accepted.
    Nokey,
    /// Pings, but nothing answers on SSH's port.
    Blocked,
    /// Refused: SSH is not running.
    Off,
    /// Tailscale SSH wants a check in a browser.
    Check,
    /// No answer at all.
    Down,
}

impl Reached {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ok" => Self::Ok,
            "nokey" => Self::Nokey,
            "blocked" => Self::Blocked,
            "off" => Self::Off,
            "check" => Self::Check,
            "down" => Self::Down,
            _ => return None,
        })
    }
}

/// Does what was found match the rule?
pub fn as_intended(expect: Expect, found: Reached) -> bool {
    match expect {
        Expect::Key => found == Reached::Ok,
        // The network works and no key opens it.
        Expect::Password => found == Reached::Nokey,
        Expect::Either => matches!(found, Reached::Ok | Reached::Nokey),
    }
}

/// A key that lets a restricted machine in is the one thing that must never be there.
pub fn breaks_rule(expect: Expect, found: Reached) -> bool {
    expect == Expect::Password && found == Reached::Ok
}

/// One link's result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub network: &'static str,
    pub expect: Expect,
    pub found: Reached,
    pub as_intended: bool,
}

/// The check, run on each machine with `id network user@address` triples as arguments. Prints
/// `id network status` per target. Needs only ssh, ping and perl. Host keys are not checked
/// and not saved: the one remote command is `true`, and BatchMode sends no password.
pub const PROBE: &str = r#"
case "$(uname)" in Darwin) ping_flags="-c 1 -t 2" ;; *) ping_flags="-c 1 -W 2" ;; esac
# The key the shortcuts use on a machine whose own key has a passphrase.
ids=""
[ -f "$HOME/.ssh/zt_ed25519" ] && ids="-i $HOME/.ssh/zt_ed25519"
probe() {
  id=$1; net=$2; dest=$3; ip=${dest#*@}
  err=$(perl -e 'alarm 14; exec @ARGV' ssh -n -o BatchMode=yes -o ConnectTimeout=6 $ids \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$dest" true 2>&1)
  rc=$?
  if [ "$rc" -eq 0 ]; then status=ok
  # Connected and then silent past ConnectTimeout: Tailscale SSH waits for a browser check,
  # and LogLevel=ERROR hides the banner saying so.
  elif [ "$rc" -eq 142 ]; then status=check
  else case "$err" in
    *"additional check"*|*"login.tailscale.com"*) status=check ;;
    *"Permission denied"*|*"Too many authentication"*) status=nokey ;;
    *refused*) status=off ;;
    *) if ping $ping_flags "$ip" >/dev/null 2>&1; then status=blocked; else status=down; fi ;;
  esac; fi
  echo "$id $net $status"
}
while [ $# -ge 3 ]; do probe "$1" "$2" "$3" & shift 3; done
wait
"#;

/// What one machine must try: every other machine on each network they share.
pub fn probe_args(me: &Node, all: &[Node]) -> Vec<String> {
    let mut out = Vec::new();
    for to in all.iter().filter(|n| n.id != me.id) {
        if me.zerotier.is_some() {
            if let Some(a) = &to.zerotier {
                out.extend([to.id.clone(), "zerotier".into(), format!("{}@{a}", to.user)]);
            }
        }
        if me.tailscale.is_some() {
            if let Some(a) = &to.tailscale {
                out.extend([
                    to.id.clone(),
                    "tailscale".into(),
                    format!("{}@{a}", to.user),
                ]);
            }
        }
    }
    out
}

/// A machine's answers to the check, as links.
pub fn parse_probe(from: &Node, all: &[Node], output: &str) -> Vec<Link> {
    let mut links: Vec<Link> = output
        .lines()
        .filter_map(|line| {
            let mut w = line.split_whitespace();
            let (id, net, status) = (w.next()?, w.next()?, w.next()?);
            let to = all.iter().find(|n| n.id == id)?;
            let network = match net {
                "zerotier" => "zerotier",
                "tailscale" => "tailscale",
                _ => return None,
            };
            let found = Reached::parse(status)?;
            let expect = expected(from, to);
            Some(Link {
                from: from.id.clone(),
                to: id.to_string(),
                network,
                expect,
                found,
                as_intended: as_intended(expect, found),
            })
        })
        .collect();
    links.sort_by(|a, b| (&a.to, a.network).cmp(&(&b.to, b.network)));
    links
}

/// The shortcuts every machine shares (`~/.ssh/config.d/kimo-mesh`).
pub fn ssh_config(all: &[Node]) -> String {
    let mut out =
        String::from("# Written by kimo-fleet from its registry: edits are overwritten.\n");
    let mut names: Vec<&str> = Vec::new();
    for n in all {
        let Some(host) = n.zerotier.as_ref().or(n.tailscale.as_ref()) else {
            continue;
        };
        names.push(&n.alias);
        out.push_str(&format!(
            "\nHost {}\n    HostName {host}\n    User {}\n",
            n.alias, n.user
        ));
        if n.restricted {
            // Never pass your keys through a machine that may not log in to the others.
            out.push_str("    ForwardAgent no\n");
        }
    }
    if !names.is_empty() {
        out.push_str(&format!(
            "\n# Save a machine's key the first time; refuse if a saved key ever changes.\nHost {}\n    StrictHostKeyChecking accept-new\n",
            names.join(" ")
        ));
        out.push_str(&format!(
            "\n# zt_ed25519: a passwordless key, on machines whose own key has a passphrase.\nMatch originalhost {} exec \"test -f ~/.ssh/zt_ed25519\"\n    IdentityFile ~/.ssh/zt_ed25519\n    IdentityFile ~/.ssh/id_ed25519\n",
            names.join(",")
        ));
    }
    out
}

/// Sets up a machine's shortcuts and prints its public key for the mesh, making one if
/// it has none it can use unattended. Run with `CONFIG` (base64) set; restricted machines run
/// it with `NOKEY=1` and get no key. Changes `~/.ssh/config` only by one `Include` line.
pub const SETUP: &str = r#"
set -eu
cd "$HOME"
mkdir -p .ssh/config.d
chmod 700 .ssh
printf %s "$CONFIG" | base64 -d > .ssh/config.d/kimo-mesh.tmp
chmod 600 .ssh/config.d/kimo-mesh.tmp
mv .ssh/config.d/kimo-mesh.tmp .ssh/config.d/kimo-mesh
if ! grep -qE '^[Ii]nclude[[:space:]]+~/.ssh/config.d/kimo-mesh' .ssh/config 2>/dev/null; then
  if [ -f .ssh/config ] && [ ! -e .ssh/config.bak-before-kimo-mesh ]; then cp -p .ssh/config .ssh/config.bak-before-kimo-mesh; fi
  { printf 'Include ~/.ssh/config.d/kimo-mesh\n\n'; [ -f .ssh/config ] && cat .ssh/config; true; } > .ssh/config.new
  chmod 600 .ssh/config.new
  mv .ssh/config.new .ssh/config
fi
ssh -G zz-kimo-check >/dev/null 2>&1 </dev/null || { echo "the ssh config does not parse" >&2; exit 4; }
[ "${NOKEY:-}" = 1 ] && exit 0
key=.ssh/id_ed25519
if [ ! -f "$key" ]; then
  ssh-keygen -q -t ed25519 -f "$key" -N "" -C "$(id -un)@$(hostname)"
elif ! ssh-keygen -y -P "" -f "$key" >/dev/null 2>&1 </dev/null; then
  key=.ssh/zt_ed25519
  [ -f "$key" ] || ssh-keygen -q -t ed25519 -f "$key" -N "" -C "zt-mesh@$(hostname)"
fi
echo "KEY $(cat "$key.pub")"
"#;

/// Sets the keys a machine accepts from the mesh: `ADD` and `REMOVE` hold base64 lines of public
/// keys. Everything else in `authorized_keys` stays. Backed up the first time.
pub const AUTHORIZE: &str = r#"
set -eu
cd "$HOME"
mkdir -p .ssh
chmod 700 .ssh
touch .ssh/authorized_keys
[ -e .ssh/authorized_keys.bak-before-kimo-mesh ] || cp -p .ssh/authorized_keys .ssh/authorized_keys.bak-before-kimo-mesh
[ -s .ssh/authorized_keys ] && [ -n "$(tail -c1 .ssh/authorized_keys)" ] && echo >> .ssh/authorized_keys
new=.ssh/authorized_keys.kimo-new
: > "$new"
chmod 600 "$new"
printf %s "$REMOVE" | base64 -d | awk '{print $2}' > "$new.drop"
while IFS= read -r line || [ -n "$line" ]; do
  body=$(printf '%s\n' "$line" | awk '{for (i=1;i<=NF;i++) if ($i ~ /^AAAA/) {print $i; exit}}')
  if [ -n "$body" ] && grep -qxF "$body" "$new.drop"; then continue; fi
  printf '%s\n' "$line" >> "$new"
done < .ssh/authorized_keys
printf %s "$ADD" | base64 -d | while IFS= read -r k; do
  [ -n "$k" ] || continue
  body=$(printf '%s\n' "$k" | awk '{print $2}')
  grep -qF "$body" "$new" || printf '%s\n' "$k" >> "$new"
done
rm -f "$new.drop"
mv "$new" .ssh/authorized_keys
"#;

/// Which keys `to` should accept and stop accepting: `(add, remove)`. `keys`: machine id →
/// public key. `known`: keys the mesh handed out earlier, for machines no longer in it or now
/// restricted. A key is only removed if the mesh handed it out, so yours stay.
pub fn keys_for<'a>(
    to: &Node,
    all: &[Node],
    keys: &'a std::collections::BTreeMap<String, String>,
    known: &'a std::collections::BTreeMap<String, String>,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let mut add = Vec::new();
    for n in all.iter().filter(|n| n.id != to.id) {
        if expected(n, to) == Expect::Key {
            if let Some(k) = keys.get(&n.id) {
                add.push(k.as_str());
            }
        }
    }
    let allowed = |id: &str| {
        all.iter()
            .find(|n| n.id == id)
            .is_some_and(|n| expected(n, to) == Expect::Key)
    };
    let remove = known
        .iter()
        .filter(|(id, _)| id.as_str() != to.id && !allowed(id))
        .map(|(_, k)| k.as_str())
        .collect();
    (add, remove)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::parse;
    use std::collections::BTreeMap;

    fn world() -> Vec<Node> {
        let r = parse(
            r#"
mesh: { restricted: [guest, nano] }
devices:
  lab: { zerotier: 10.144.10.11, tailscale: lab-ts, user: demo, alias: zlab }
  pi: { kimo: "http://10.144.10.12:7777", zerotier: 10.144.10.12, user: demo, alias: zpi }
  guest: { zerotier: 10.144.10.13, user: demo, alias: guest }
  nano: { zerotier: 10.144.10.14, user: edge, alias: edge }
  phone: { tailscale: pixel }
"#,
        )
        .unwrap();
        nodes(&r)
    }

    fn node<'a>(all: &'a [Node], id: &str) -> &'a Node {
        all.iter().find(|n| n.id == id).unwrap()
    }

    #[test]
    fn only_machines_with_an_account_are_in_it() {
        let all = world();
        assert_eq!(all.len(), 4);
        assert_eq!(node(&all, "lab").target(), "demo@10.144.10.11");
        assert!(node(&all, "guest").restricted);
    }

    #[test]
    fn restricted_machines_can_be_reached_but_cannot_reach() {
        let all = world();
        let (lab, pi, guest, nano) = (
            node(&all, "lab"),
            node(&all, "pi"),
            node(&all, "guest"),
            node(&all, "nano"),
        );
        assert_eq!(expected(lab, pi), Expect::Key);
        assert_eq!(expected(lab, guest), Expect::Key);
        assert_eq!(expected(guest, lab), Expect::Password);
        assert_eq!(expected(guest, nano), Expect::Either);
        assert!(as_intended(Expect::Password, Reached::Nokey));
        assert!(!as_intended(Expect::Password, Reached::Ok));
        assert!(breaks_rule(Expect::Password, Reached::Ok));
        assert!(!breaks_rule(Expect::Key, Reached::Ok));
        assert!(!as_intended(Expect::Key, Reached::Down));
        assert!(as_intended(Expect::Either, Reached::Ok));
    }

    #[test]
    fn the_check_tries_each_shared_network() {
        let all = world();
        let args = probe_args(node(&all, "pi"), &all);
        assert_eq!(args.len(), 9); // lab, guest, nano over ZeroTier; the Pi has no Tailscale
        assert_eq!(&args[0..3], ["guest", "zerotier", "demo@10.144.10.13"]);
        let lab_args = probe_args(node(&all, "lab"), &all);
        assert!(!lab_args.contains(&"tailscale".to_string())); // nobody else has it
    }

    #[test]
    fn answers_become_links() {
        let all = world();
        let out = "pi zerotier ok\nguest zerotier nokey\nnobody zerotier ok\npi moon ok\npi tailscale what\nnano tailscale check\n";
        let links = parse_probe(node(&all, "lab"), &all, out);
        assert_eq!(links.len(), 3);
        assert_eq!(
            (links[0].to.as_str(), links[0].found),
            ("guest", Reached::Nokey)
        );
        assert!(!links[0].as_intended);
        assert_eq!(links[2].network, "zerotier");
        assert!(links[2].as_intended);
        let from_guest = parse_probe(
            node(&all, "guest"),
            &all,
            "lab zerotier nokey\nlab tailscale ok",
        );
        assert!(from_guest[1].as_intended || from_guest[0].as_intended);
        assert!(breaks_rule(from_guest[0].expect, from_guest[0].found));
    }

    #[test]
    fn the_shortcuts_file() {
        let text = ssh_config(&world());
        assert!(text.contains("Host zlab\n    HostName 10.144.10.11\n    User demo\n"));
        assert!(text.contains(
            "Host edge\n    HostName 10.144.10.14\n    User edge\n    ForwardAgent no\n"
        ));
        assert!(text.contains("Host guest zlab edge zpi\n"));
        assert!(text.contains("exec \"test -f ~/.ssh/zt_ed25519\""));
        assert!(!text.contains("pixel"));
    }

    #[test]
    fn keys_follow_the_rule() {
        let all = world();
        let keys: BTreeMap<String, String> =
            [("lab", "ssh-ed25519 LAB lab"), ("pi", "ssh-ed25519 PI pi")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .into();
        let mut known = keys.clone();
        known.insert("sold".into(), "ssh-ed25519 SOLD old".into());
        known.insert("guest".into(), "ssh-ed25519 GUEST old".into());
        // A personal machine accepts the other personal ones; never its own, the sold one's
        // or a restricted one's.
        let (add, remove) = keys_for(node(&all, "lab"), &all, &keys, &known);
        assert_eq!(add, ["ssh-ed25519 PI pi"]);
        assert!(remove.contains(&"ssh-ed25519 SOLD old"));
        assert!(remove.contains(&"ssh-ed25519 GUEST old"));
        assert!(!remove.contains(&"ssh-ed25519 LAB lab"));
        assert!(!remove.contains(&"ssh-ed25519 PI pi"));
        // A restricted machine can be logged in to by personal machines.
        let (add, _) = keys_for(node(&all, "guest"), &all, &keys, &known);
        assert_eq!(add.len(), 2);
    }

    #[test]
    fn the_scripts_are_valid_sh() {
        for script in [PROBE, SETUP, AUTHORIZE] {
            let out = std::process::Command::new("sh")
                .args(["-n", "-c", script])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn authorize_touches_only_what_it_should() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        std::fs::write(
            ssh.join("authorized_keys"),
            "ssh-ed25519 AAAAyours you@laptop\ncommand=\"x\" ssh-ed25519 AAAAforced tool\nssh-ed25519 AAAAsold old@sold\n",
        )
        .unwrap();
        use base64::Engine;
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let run = || {
            let out = std::process::Command::new("sh")
                .args(["-c", AUTHORIZE])
                .env("HOME", home.path())
                .env("ADD", b64("ssh-ed25519 AAAAnew new@lab\n"))
                .env("REMOVE", b64("ssh-ed25519 AAAAsold old@sold\n"))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run();
        run(); // twice changes nothing
        let text = std::fs::read_to_string(ssh.join("authorized_keys")).unwrap();
        assert_eq!(
            text,
            "ssh-ed25519 AAAAyours you@laptop\ncommand=\"x\" ssh-ed25519 AAAAforced tool\nssh-ed25519 AAAAnew new@lab\n"
        );
        assert!(ssh.join("authorized_keys.bak-before-kimo-mesh").exists());
    }

    #[test]
    fn setup_adds_one_include_and_a_key() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host old\n    HostName 1.2.3.4\n").unwrap();
        use base64::Engine;
        let config = base64::engine::general_purpose::STANDARD.encode(ssh_config(&world()));
        for _ in 0..2 {
            let out = std::process::Command::new("sh")
                .args(["-c", SETUP])
                .env("HOME", home.path())
                .env("CONFIG", &config)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(stdout.starts_with("KEY ssh-ed25519 "), "{stdout}");
        }
        let text = std::fs::read_to_string(ssh.join("config")).unwrap();
        assert_eq!(text.matches("Include").count(), 1);
        assert!(text.ends_with("Host old\n    HostName 1.2.3.4\n"));
        assert!(ssh.join("config.bak-before-kimo-mesh").exists());
    }
}
