//! TPAP, how TP-Link Tapo plugs talk since firmware 1.4 (a P110 reports it in discovery as
//! `encrypt_type: "TPAP"`, and refuses the older KLAP handshake): a SPAKE2+ login on P-256,
//! then JSON commands in AES-128-CCM. Unofficial: TP-Link publishes no local API, and a
//! firmware update can change this. Written from the behaviour of real plugs and two MIT
//! references (rabilrbl/tapo-p110-ha, carboni123/tapo-local); `tests` holds a vector made with
//! the first one.
//!
//! Pure: the network is in `services::plugs`.

use aes::Aes128;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{Aead, KeyInit};
use ccm::consts::{U12, U16};
use ccm::Ccm;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use p256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use p256::elliptic_curve::Field;
use p256::{AffinePoint, EncodedPoint, ProjectivePoint, Scalar};
use serde::Serialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};

/// The name every plug logs in with: only the password counts.
pub const USERNAME: &str = "21232f297a57a5a743894a0e4a801fc3"; // md5("admin")

/// SPAKE2+ P-256 points M and N (RFC 9383).
const M: [u8; 33] = hex33("02886e2f97ace46e55ba9dd7242579f2993b64e16ef3dcab95afd497333d8fa12f");
const N: [u8; 33] = hex33("03d8bbd6c639c62937b04d997f38c3770719c629d7014d49a24b4f98baa1292b49");

const fn hex_digit(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        _ => c - b'a' + 10,
    }
}

const fn hex33(s: &str) -> [u8; 33] {
    let b = s.as_bytes();
    let mut out = [0u8; 33];
    let mut i = 0;
    while i < 33 {
        out[i] = hex_digit(b[i * 2]) << 4 | hex_digit(b[i * 2 + 1]);
        i += 1;
    }
    out
}

fn point(bytes: &[u8]) -> Result<ProjectivePoint, String> {
    let encoded = EncodedPoint::from_bytes(bytes).map_err(|_| "not a curve point".to_string())?;
    let affine: Option<AffinePoint> = AffinePoint::from_encoded_point(&encoded).into();
    affine
        .map(ProjectivePoint::from)
        .ok_or_else(|| "not a point on the curve".to_string())
}

fn encode(p: &ProjectivePoint) -> Vec<u8> {
    p.to_affine().to_encoded_point(false).as_bytes().to_vec()
}

/// A big-endian number of any length, modulo the curve's order.
fn reduce(bytes: &[u8]) -> Scalar {
    let base = Scalar::from(256u64);
    bytes.iter().fold(Scalar::ZERO, |acc, b| {
        acc * base + Scalar::from(u64::from(*b))
    })
}

/// What is kept instead of the password: the plug's `password_shadow` for `passwd_id: 2` is
/// the SHA-1 of the TP-Link account password, in lower-case hex.
pub fn credential(password: &str) -> String {
    let mut h = Sha1::new();
    h.update(password.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh login's secret and the context both sides hash.
pub struct Login {
    w: Scalar,
    h: Scalar,
    x: Scalar,
    context: [u8; 32],
    /// Our share to send (`user_share`).
    pub share: Vec<u8>,
}

/// What a login agrees on.
pub struct Agreed {
    pub user_confirm: [u8; 32],
    pub dev_confirm: [u8; 32],
    pub cipher: Cipher,
}

impl Login {
    /// From `credential()`, the plug's salt, iterations and random, and ours.
    pub fn new(
        credential: &str,
        salt: &[u8],
        iterations: u32,
        user_random: &[u8],
        dev_random: &[u8],
    ) -> Result<Self, String> {
        Self::with_secret(
            credential,
            salt,
            iterations,
            user_random,
            dev_random,
            Scalar::random(&mut rand_core::OsRng),
        )
    }

    fn with_secret(
        credential: &str,
        salt: &[u8],
        iterations: u32,
        user_random: &[u8],
        dev_random: &[u8],
        x: Scalar,
    ) -> Result<Self, String> {
        let mut derived = [0u8; 80];
        pbkdf2::pbkdf2_hmac::<Sha256>(credential.as_bytes(), salt, iterations, &mut derived);
        let w = reduce(&derived[..40]);
        let h = reduce(&derived[40..]);
        let m = point(&M)?;
        let share = ProjectivePoint::GENERATOR * x + m * w;
        let mut context = Sha256::new();
        context.update(b"PAKE V1");
        context.update(user_random);
        context.update(dev_random);
        Ok(Self {
            w,
            h,
            x,
            context: context.finalize().into(),
            share: encode(&share),
        })
    }

    /// Would the two ways plugs may hash `w` differ for this login? (Then a refused login is
    /// worth one more try with `full_width`.)
    pub fn ambiguous(&self) -> bool {
        self.w.to_bytes()[0] == 0
    }

    /// With the plug's share: the confirmations and the session. `full_width`: hash `w` as 32
    /// bytes rather than its shortest even form; only differs when `w` starts with a zero byte.
    pub fn finish(&self, dev_share: &[u8], full_width: bool) -> Result<Agreed, String> {
        let y = point(dev_share)?;
        let (m, n) = (point(&M)?, point(&N)?);
        let unblinded = y - n * self.w;
        let w_bytes: [u8; 32] = self.w.to_bytes().into();
        let w_field = if full_width {
            w_bytes.to_vec()
        } else {
            short_form(&w_bytes)
        };
        let mut t = Sha256::new();
        let parts: [&[u8]; 10] = [
            &self.context,
            b"",
            b"",
            &encode(&m),
            &encode(&n),
            &self.share,
            &encode(&y),
            &encode(&(unblinded * self.x)),
            &encode(&(unblinded * self.h)),
            &w_field,
        ];
        for part in parts {
            t.update((part.len() as u64).to_le_bytes());
            t.update(part);
        }
        let transcript = t.finalize();
        let kdf = Hkdf::<Sha256>::new(None, &transcript);
        let (mut confirmation, mut shared) = ([0u8; 64], [0u8; 32]);
        kdf.expand(b"ConfirmationKeys", &mut confirmation)
            .map_err(|e| e.to_string())?;
        kdf.expand(b"SharedKey", &mut shared)
            .map_err(|e| e.to_string())?;
        Ok(Agreed {
            user_confirm: hmac(&confirmation[..32], &encode(&y))?,
            dev_confirm: hmac(&confirmation[32..], &self.share)?,
            cipher: Cipher::new(&shared)?,
        })
    }
}

/// `w` in its shortest even-length form, with a zero in front when the top bit would be set.
fn short_form(w: &[u8; 32]) -> Vec<u8> {
    let first = w.iter().position(|b| *b != 0).unwrap_or(31);
    let bytes = &w[first..];
    if bytes.len().is_multiple_of(2) {
        return bytes.to_vec();
    }
    if bytes[0] & 0x80 != 0 {
        let mut out = vec![0u8];
        out.extend_from_slice(bytes);
        return out;
    }
    bytes.to_vec()
}

fn hmac(key: &[u8], data: &[u8]) -> Result<[u8; 32], String> {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|e| e.to_string())?;
    m.update(data);
    Ok(m.finalize().into_bytes().into())
}

/// The session's cipher: AES-128-CCM with a 16-byte tag; the sequence number is the end of
/// the nonce and goes in clear in front of each message.
pub struct Cipher {
    key: [u8; 16],
    nonce: [u8; 12],
}

impl Cipher {
    fn new(shared: &[u8; 32]) -> Result<Self, String> {
        let (mut key, mut nonce) = ([0u8; 16], [0u8; 12]);
        Hkdf::<Sha256>::new(Some(b"tp-kdf-salt-aes128-key"), shared)
            .expand(b"tp-kdf-info-aes128-key", &mut key)
            .map_err(|e| e.to_string())?;
        Hkdf::<Sha256>::new(Some(b"tp-kdf-salt-aes128-iv"), shared)
            .expand(b"tp-kdf-info-aes128-iv", &mut nonce)
            .map_err(|e| e.to_string())?;
        Ok(Self { key, nonce })
    }

    fn at(&self, seq: u32) -> [u8; 12] {
        let mut nonce = self.nonce;
        nonce[8..].copy_from_slice(&seq.to_be_bytes());
        nonce
    }

    /// `seq` (4 bytes) then the sealed message.
    pub fn seal(&self, seq: u32, plain: &[u8]) -> Result<Vec<u8>, String> {
        let aead = Ccm::<Aes128, U16, U12>::new(GenericArray::from_slice(&self.key));
        let nonce = self.at(seq);
        let sealed = aead
            .encrypt(GenericArray::from_slice(&nonce), plain)
            .map_err(|_| "could not encrypt".to_string())?;
        let mut out = seq.to_be_bytes().to_vec();
        out.extend(sealed);
        Ok(out)
    }

    /// Back from `seal`: the plain message.
    pub fn open(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        if message.len() < 4 + 16 {
            return Err("the plug's answer is too short".into());
        }
        let seq = u32::from_be_bytes(message[..4].try_into().expect("four bytes"));
        let aead = Ccm::<Aes128, U16, U12>::new(GenericArray::from_slice(&self.key));
        let nonce = self.at(seq);
        aead.decrypt(GenericArray::from_slice(&nonce), &message[4..])
            .map_err(|_| "could not decrypt the plug's answer".to_string())
    }
}

/// A command the plug understands, as JSON.
pub fn command(method: &str, params: Option<serde_json::Value>) -> Vec<u8> {
    let mut v = serde_json::json!({ "method": method });
    if let Some(p) = params {
        v["params"] = p;
    }
    serde_json::to_vec(&v).unwrap_or_default()
}

/// A plug's error codes, in plain words where they mean something to its owner.
pub fn error_text(code: i64) -> String {
    match code {
        -1501 | -2202 | -2203 | -1503 => {
            "the plug refused the password: set your TP-Link account's with `kimo-fleet plug-login`"
                .into()
        }
        1003 => "the plug does not understand this request (its firmware may have changed)".into(),
        9999 => "the plug's session ended".into(),
        c => format!("the plug answered with error {c}"),
    }
}

/// What a plug reports, reduced to what the fleet shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PlugStatus {
    pub on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watts: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today_wh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month_wh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// How long the relay has been on, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_seconds: Option<u64>,
}

/// From `get_device_info` and, when the plug meters, `get_energy_usage`.
pub fn plug_status(info: &serde_json::Value, energy: Option<&serde_json::Value>) -> PlugStatus {
    let text = |k: &str| info[k].as_str().map(str::to_string);
    PlugStatus {
        on: info["device_on"] == true,
        // Milliwatts in get_energy_usage.
        watts: energy
            .and_then(|e| e["current_power"].as_f64())
            .map(|mw| (mw / 100.0).round() / 10.0),
        today_wh: energy.and_then(|e| e["today_energy"].as_f64()),
        month_wh: energy.and_then(|e| e["month_energy"].as_f64()),
        model: text("model"),
        firmware: text("fw_ver"),
        on_seconds: info["on_time"].as_u64(),
    }
}

/// What can be done with a machine's plug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlugAction {
    On,
    Off,
    /// Off, then on again: a machine set to start when power returns boots.
    Cycle,
}

/// What is known when asking to cut a machine's power.
#[derive(Debug, Clone, Default)]
pub struct CutCheck {
    /// The plug is reached through the machine it powers (`plug.via` is itself).
    pub reached_through_itself: bool,
    /// The coordinator runs on this machine.
    pub hosts_coordinator: bool,
    /// Why it should stay on: jobs, agents, logins; as words.
    pub blockers: Vec<String>,
    /// It looks on, and nothing could say what runs there.
    pub unknown_load: bool,
}

/// May this happen? `Err` says why not; `force` passes only what a person can overrule.
/// Why cutting the power was refused. `overrule`: `force` would allow it (the API's 409).
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub why: String,
    pub overrule: bool,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl std::error::Error for Refused {}

pub fn may_act(action: PlugAction, check: &CutCheck, force: bool) -> Result<(), Refused> {
    let refuse = |why: String, overrule: bool| Err(Refused { why, overrule });
    if action == PlugAction::On {
        return Ok(());
    }
    if action == PlugAction::Off && check.reached_through_itself {
        return refuse(
            "this plug is reached through the machine it powers: nothing could switch it on again, \
             so it can only be restarted"
                .into(),
            false,
        );
    }
    if action == PlugAction::Off && check.hosts_coordinator {
        return refuse(
            "the coordinator runs on this machine: it can only be restarted".into(),
            false,
        );
    }
    if force {
        return Ok(());
    }
    if !check.blockers.is_empty() {
        return refuse(
            format!(
                "cutting the power now would lose work: {}",
                check.blockers.join(", ")
            ),
            true,
        );
    }
    if check.unknown_load {
        return refuse(
            "it looks on and has no kimo to ask what runs there: cutting the power could lose work"
                .into(),
            true,
        );
    }
    Ok(())
}

/// The plug's side of the login, for a stand-in plug in tests.
#[cfg(test)]
pub(crate) mod device {
    use super::*;

    pub struct Device {
        w: Scalar,
        h: Scalar,
        y: Scalar,
        context: [u8; 32],
        pub share: Vec<u8>,
    }

    impl Device {
        pub fn new(
            credential: &str,
            salt: &[u8],
            iterations: u32,
            user_random: &[u8],
            dev_random: &[u8],
        ) -> Self {
            let mut derived = [0u8; 80];
            pbkdf2::pbkdf2_hmac::<Sha256>(credential.as_bytes(), salt, iterations, &mut derived);
            let (w, h) = (reduce(&derived[..40]), reduce(&derived[40..]));
            let y = Scalar::random(&mut rand_core::OsRng);
            let n = point(&N).unwrap();
            let share = encode(&(ProjectivePoint::GENERATOR * y + n * w));
            let mut context = Sha256::new();
            context.update(b"PAKE V1");
            context.update(user_random);
            context.update(dev_random);
            Self {
                w,
                h,
                y,
                context: context.finalize().into(),
                share,
            }
        }

        /// With the client's share: what both sides should have agreed.
        pub fn finish(&self, user_share: &[u8]) -> Agreed {
            let x_pt = point(user_share).unwrap();
            let (m, n) = (point(&M).unwrap(), point(&N).unwrap());
            let z = (x_pt - m * self.w) * self.y;
            let v = ProjectivePoint::GENERATOR * self.h * self.y;
            let w_bytes: [u8; 32] = self.w.to_bytes().into();
            let w_field = short_form(&w_bytes);
            let mut t = Sha256::new();
            let parts: [&[u8]; 10] = [
                &self.context,
                b"",
                b"",
                &encode(&m),
                &encode(&n),
                user_share,
                &self.share,
                &encode(&z),
                &encode(&v),
                &w_field,
            ];
            for part in parts {
                t.update((part.len() as u64).to_le_bytes());
                t.update(part);
            }
            let transcript = t.finalize();
            let kdf = Hkdf::<Sha256>::new(None, &transcript);
            let (mut confirmation, mut shared) = ([0u8; 64], [0u8; 32]);
            kdf.expand(b"ConfirmationKeys", &mut confirmation).unwrap();
            kdf.expand(b"SharedKey", &mut shared).unwrap();
            Agreed {
                user_confirm: hmac(&confirmation[..32], &self.share).unwrap(),
                dev_confirm: hmac(&confirmation[32..], user_share).unwrap(),
                cipher: Cipher::new(&shared).unwrap(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::elliptic_curve::PrimeField;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    fn scalar(s: &str) -> Scalar {
        let bytes: [u8; 32] = unhex(s).try_into().unwrap();
        Option::from(Scalar::from_repr(bytes.into())).unwrap()
    }

    // Made with the MIT reference's algorithm (rabilrbl/tapo-p110-ha) and fixed inputs.
    const X_SECRET: &str = "948fe603f61dc036b5c596dc09fe3ce3f3d30dc90f024c85f3c82db2ccab679d";
    const X_SHARE: &str = "04e98ac01a4f54d983490556eedbd16ee131db9315e7a44919c6dc74b1901f1aa3cada097208eeaaeca8e4f30c0c6890437c87e17d8a0a760f7071bb0b83d02387";
    const Y_SHARE: &str = "04911893e4f86fa803f9523772434d7fa0e31ade9f60c0c9218e86af2551df005c889ca1d19fa9a6e6b6807c328625002c7da9147c387751148e96a0e1f87b9f79";
    const USER_CONFIRM: &str = "8a7e2353d93f0e962dcc12deae4c479a65c14ecc7de4e37b4cc4f356c90e1608";
    const DEV_CONFIRM: &str = "27a3d86879dc0ef82907d20c7fb63ef51f7c8adff52c979409cffba7fb2cc0cf";
    const SEALED: &str =
        "890bea3b1bb8312b9edf27017b1b0fc40f83d2156280b34a25888c6a64f0cf591273db1d46b546a76406d4f6";

    fn login() -> Login {
        let salt: Vec<u8> = (0..16).collect();
        Login::with_secret(
            &credential("correct horse"),
            &salt,
            1000,
            &[1u8; 32],
            &[2u8; 32],
            scalar(X_SECRET),
        )
        .unwrap()
    }

    #[test]
    fn the_reference_vector() {
        let l = login();
        assert_eq!(l.share, unhex(X_SHARE));
        let a = l.finish(&unhex(Y_SHARE), false).unwrap();
        assert_eq!(a.user_confirm.to_vec(), unhex(USER_CONFIRM));
        assert_eq!(a.dev_confirm.to_vec(), unhex(DEV_CONFIRM));
        let plain = br#"{"method":"get_device_info"}"#;
        let sealed = a.cipher.seal(7, plain).unwrap();
        assert_eq!(&sealed[..4], &[0, 0, 0, 7]);
        assert_eq!(sealed[4..].to_vec(), unhex(SEALED));
        assert_eq!(a.cipher.open(&sealed).unwrap(), plain);
    }

    #[test]
    fn a_tampered_answer_is_refused() {
        let a = login().finish(&unhex(Y_SHARE), false).unwrap();
        let mut sealed = a.cipher.seal(1, b"{}").unwrap();
        *sealed.last_mut().unwrap() ^= 1;
        assert!(a.cipher.open(&sealed).is_err());
        assert!(a.cipher.open(&[0, 0, 0]).is_err());
    }

    #[test]
    fn a_share_off_the_curve_is_refused() {
        assert!(login().finish(&[4u8; 65], false).is_err());
        assert!(login().finish(b"nonsense", false).is_err());
    }

    #[test]
    fn the_short_form_of_w() {
        let mut w = [0x11u8; 32];
        assert_eq!(short_form(&w).len(), 32);
        w[0] = 0; // 31 bytes, top bit clear: stays odd
        assert_eq!(short_form(&w).len(), 31);
        w[1] = 0x80; // 31 bytes, top bit set: a zero in front
        let s = short_form(&w);
        assert_eq!((s.len(), s[0], s[1]), (32, 0, 0x80));
    }

    #[test]
    fn the_credential_is_sha1_hex() {
        assert_eq!(
            credential("abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn status_from_real_field_names() {
        let info = serde_json::json!({ "device_on": true, "model": "P110", "fw_ver": "1.4.3", "on_time": 90 });
        let energy = serde_json::json!({ "current_power": 12345, "today_energy": 200, "month_energy": 4000 });
        let s = plug_status(&info, Some(&energy));
        assert!(s.on);
        assert_eq!(s.watts, Some(12.3));
        assert_eq!((s.today_wh, s.month_wh), (Some(200.0), Some(4000.0)));
        assert_eq!(s.on_seconds, Some(90));
        assert_eq!(plug_status(&info, None).watts, None);
    }

    #[test]
    fn cutting_power_is_guarded() {
        use PlugAction::*;
        let free = CutCheck::default();
        assert!(may_act(Off, &free, false).is_ok());
        assert!(may_act(Cycle, &free, false).is_ok());

        let busy = CutCheck {
            blockers: vec!["2 jobs".into()],
            ..Default::default()
        };
        let refused = may_act(Cycle, &busy, false).unwrap_err();
        assert!(refused.why.contains("2 jobs") && refused.overrule);
        assert!(may_act(Off, &busy, true).is_ok());
        assert!(may_act(On, &busy, false).is_ok());

        let unknown = CutCheck {
            unknown_load: true,
            ..Default::default()
        };
        assert!(may_act(Off, &unknown, false).is_err());
        assert!(may_act(Off, &unknown, true).is_ok());

        // Force never strands a machine.
        let itself = CutCheck {
            reached_through_itself: true,
            ..Default::default()
        };
        assert!(!may_act(Off, &itself, true).unwrap_err().overrule);
        assert!(may_act(Cycle, &itself, false).is_ok());
        let coordinator = CutCheck {
            hosts_coordinator: true,
            ..Default::default()
        };
        assert!(may_act(Off, &coordinator, true).is_err());
        assert!(may_act(Cycle, &coordinator, false).is_ok());
    }
}
