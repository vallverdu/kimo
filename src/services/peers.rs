//! Other kimo installations paired with this one (both ways), so one app and every agent can
//! reach the agents on all of them: `gpu-lab/api-1a2b`.

use crate::client::Client;
use crate::core::hostname::{host_slug, split_remote};
use crate::core::models::Peer;
use crate::services::sessions::Manager;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub url: String,
    pub self_name: String,
    pub online: bool,
    pub sessions: Option<usize>,
}

impl Manager {
    pub fn peer_list(&self) -> Vec<Peer> {
        self.lock().state.peers.clone()
    }

    pub fn peer(&self, id: &str) -> Option<Peer> {
        self.lock().state.peers.iter().find(|p| p.id == id).cloned()
    }

    pub fn peer_client(peer: &Peer) -> Client {
        Client::new(&peer.url, Some(peer.token.clone()))
    }

    /// Peers with whether they answer right now (checked in parallel, 3 s at most).
    pub fn peers(&self) -> Vec<PeerInfo> {
        let peers = self.peer_list();
        let handles: Vec<_> = peers
            .into_iter()
            .map(|p| {
                std::thread::spawn(move || {
                    let sessions = Self::peer_client(&p)
                        .call_timeout("GET", "/sessions", None, 3)
                        .ok()
                        .and_then(|v| v.as_array().map(Vec::len));
                    PeerInfo {
                        id: p.id,
                        name: p.name,
                        url: p.url,
                        self_name: p.self_name,
                        online: sessions.is_some(),
                        sessions,
                    }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    }

    fn unique_peer_id(&self, name: &str) -> String {
        let base = host_slug(name);
        let taken: Vec<String> = self
            .lock()
            .state
            .peers
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let mut id = base.clone();
        let mut n = 2;
        while taken.contains(&id) || id == host_slug(self.host_name()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Pair with another kimo using a pairing code shown there (QR, Devices, `kimo pair`).
    /// `my_url` is where the other kimo can reach this one. Both sides end up knowing each other.
    pub fn add_peer(&self, url: &str, code: &str, my_url: &str) -> Result<Peer> {
        let url = url.trim().trim_end_matches('/').to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            bail!("the address must start with http:// or https://");
        }
        if self.lock().state.peers.iter().any(|p| p.url == url) {
            bail!("already paired with {url}");
        }
        let open = Client::new(&url, None);
        let status = open
            .call_timeout(
                "POST",
                "/pair",
                Some(
                    &json!({ "code": code.trim(), "name": format!("{} (kimo)", self.host_name()) }),
                ),
                10,
            )
            .with_context(|| format!("pairing with {url}"))?;
        let token = status["token"]
            .as_str()
            .ok_or_else(|| anyhow!("{url} did not return a key"))?
            .to_string();
        let remote = Client::new(&url, Some(token.clone()));
        let info = remote.call_timeout("GET", "/status", None, 10)?;
        let remote_name = info["host"].as_str().unwrap_or("kimo").to_string();
        // A key for the other side, and register ourselves there.
        let back = self.create_device(&format!("{remote_name} (kimo)"))?;
        let accepted = remote.call_timeout(
            "POST",
            "/peers/accept",
            Some(&json!({ "name": self.host_name(), "url": my_url, "token": back.token })),
            10,
        )?;
        let peer = Peer {
            id: self.unique_peer_id(&remote_name),
            name: remote_name,
            url,
            token,
            // The prefix under which the other kimo lists our agents.
            self_name: accepted["accepted_as"]
                .as_str()
                .unwrap_or(&host_slug(self.host_name()))
                .to_string(),
            device: Some(back.id),
            extra: Default::default(),
        };
        let mut inner = self.lock();
        inner.state.peers.push(peer.clone());
        self.save(&inner)?;
        Ok(peer)
    }

    /// The other half of mutual pairing: a peer registers itself with a key for it.
    /// `device`: the device (key) the peer calls this kimo with, revoked when it is forgotten.
    pub fn accept_peer(
        &self,
        name: &str,
        url: &str,
        token: &str,
        device: Option<String>,
    ) -> Result<Peer> {
        let url = url.trim().trim_end_matches('/').to_string();
        let mut inner = self.lock();
        // Pairing again replaces the old record; the old key goes with it.
        let old: Vec<Peer> = inner
            .state
            .peers
            .iter()
            .filter(|p| p.url == url)
            .cloned()
            .collect();
        inner.state.peers.retain(|p| p.url != url);
        drop(inner);
        for p in old
            .iter()
            .filter(|p| p.device.is_some() && p.device != device)
        {
            self.revoke_peer_key(p);
        }
        let peer = Peer {
            id: self.unique_peer_id(name),
            name: name.to_string(),
            url,
            token: token.to_string(),
            self_name: host_slug(self.host_name()),
            device,
            extra: Default::default(),
        };
        let mut inner = self.lock();
        inner.state.peers.push(peer.clone());
        self.save(&inner)?;
        Ok(peer)
    }

    /// Forget a peer and revoke the key it uses on this kimo (its open terminals close).
    /// Asking the peer to forget this kimo too is the API's part.
    pub fn remove_peer(&self, id: &str) -> Result<Peer> {
        let mut inner = self.lock();
        let peer = inner
            .state
            .peers
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown host: {id}"))?;
        inner.state.peers.retain(|p| p.id != id);
        self.save(&inner)?;
        drop(inner);
        self.revoke_peer_key(&peer);
        Ok(peer)
    }

    /// Revoke the device a peer uses here: the one recorded at pairing, or (pairings made
    /// before kimo recorded it) the one pairing named after the peer.
    fn revoke_peer_key(&self, peer: &Peer) {
        let devices: Vec<String> = match &peer.device {
            Some(id) => vec![id.clone()],
            None => {
                let name = format!("{} (kimo)", peer.name);
                self.lock()
                    .state
                    .devices
                    .iter()
                    .filter(|d| d.name == name)
                    .map(|d| d.id.clone())
                    .collect()
            }
        };
        for id in devices {
            if let Err(e) = self.delete_device(&id) {
                tracing::info!("key of {} already gone: {e:#}", peer.name);
            }
        }
    }

    /// Sessions of this kimo and of every reachable peer (ids of remote ones are `peer/id`).
    pub fn all_sessions(&self) -> Vec<Value> {
        let host = self.host_name().to_string();
        let mut all: Vec<Value> = self
            .sessions()
            .into_iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                v["host"] = json!(host);
                v["local"] = json!(true);
                v
            })
            .collect();
        let handles: Vec<_> = self
            .peer_list()
            .into_iter()
            .map(|p| {
                std::thread::spawn(move || {
                    let list = Self::peer_client(&p)
                        .call_timeout("GET", "/sessions", None, 3)
                        .ok()?;
                    Some(
                        list.as_array()?
                            .iter()
                            .map(|s| {
                                let mut v = s.clone();
                                v["id"] =
                                    json!(format!("{}/{}", p.id, s["id"].as_str().unwrap_or("")));
                                v["host"] = json!(p.name);
                                v["local"] = json!(false);
                                v
                            })
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect();
        for h in handles {
            if let Ok(Some(list)) = h.join() {
                all.extend(list);
            }
        }
        all
    }

    /// `peer/id` → the peer and the id over there.
    pub fn remote(&self, id: &str) -> Option<(Peer, String)> {
        let (host, rest) = split_remote(id)?;
        Some((self.peer(host)?, rest.to_string()))
    }
}
