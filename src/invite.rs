//! Ephemeral room invites exchanged out-of-band (e.g. via Discord chat).
//!
//! The encoded token is a bearer secret: anyone holding it can attempt to
//! join the room while it is open. It is never logged in full; use
//! [`Invite::redacted`] for status lines, metrics and errors.

use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};

use crate::protocol::PROTOCOL_VERSION;

pub const INVITE_PREFIX: &str = "rh1.";
pub const MAX_INVITE_CHARS: usize = 2048;
pub const ROOM_BYTES: usize = 16;
pub const PSK_BYTES: usize = 32;
pub const DEFAULT_INVITE_TTL_SECS: u64 = 900;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    version: u16,
    room: [u8; ROOM_BYTES],
    psk: [u8; PSK_BYTES],
    expires_at: u64,
}

#[derive(Serialize, Deserialize)]
struct InviteBody {
    v: u16,
    room: String,
    psk: String,
    exp: u64,
}

impl Invite {
    pub fn create(now_secs: u64, ttl_secs: u64) -> Self {
        let mut room = [0u8; ROOM_BYTES];
        let mut psk = [0u8; PSK_BYTES];
        rand::rngs::OsRng
            .try_fill_bytes(&mut room)
            .expect("OS RNG disponível");
        rand::rngs::OsRng
            .try_fill_bytes(&mut psk)
            .expect("OS RNG disponível");
        Self {
            version: PROTOCOL_VERSION,
            room,
            psk,
            expires_at: now_secs.saturating_add(ttl_secs.clamp(60, 3600)),
        }
    }

    #[cfg(test)]
    fn fixed_for_tests() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            room: [7u8; ROOM_BYTES],
            psk: [9u8; PSK_BYTES],
            expires_at: 1_700_000_000,
        }
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs > self.expires_at
    }

    /// Short non-secret identifier safe for logs and UI status lines.
    pub fn redacted(&self) -> String {
        format!("rh1.…{}", hex_tail(&self.room))
    }

    pub fn encode(&self) -> String {
        let body = InviteBody {
            v: self.version,
            room: URL_SAFE_NO_PAD.encode(self.room),
            psk: URL_SAFE_NO_PAD.encode(self.psk),
            exp: self.expires_at,
        };
        let json = serde_json::to_vec(&body).expect("invite serializável");
        format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(token: &str, now_secs: u64) -> Result<Self> {
        ensure!(
            !token.is_empty() && token.len() <= MAX_INVITE_CHARS,
            "convite inválido"
        );
        let body = token
            .strip_prefix(INVITE_PREFIX)
            .ok_or_else(|| anyhow::anyhow!("convite inválido"))?;
        ensure!(
            !body.is_empty() && body.len() <= MAX_INVITE_CHARS,
            "convite inválido"
        );
        let json = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| anyhow::anyhow!("convite inválido"))?;
        ensure!(json.len() <= MAX_INVITE_CHARS, "convite inválido");
        let body: InviteBody =
            serde_json::from_slice(&json).map_err(|_| anyhow::anyhow!("convite inválido"))?;
        ensure!(body.v == PROTOCOL_VERSION, "versão de convite desconhecida");
        let room = URL_SAFE_NO_PAD
            .decode(&body.room)
            .map_err(|_| anyhow::anyhow!("convite inválido"))?;
        let psk = URL_SAFE_NO_PAD
            .decode(&body.psk)
            .map_err(|_| anyhow::anyhow!("convite inválido"))?;
        ensure!(
            room.len() == ROOM_BYTES && psk.len() == PSK_BYTES,
            "convite inválido"
        );
        ensure!(body.exp > 0, "convite inválido");
        let mut room_arr = [0u8; ROOM_BYTES];
        room_arr.copy_from_slice(&room);
        let mut psk_arr = [0u8; PSK_BYTES];
        psk_arr.copy_from_slice(&psk);
        let invite = Self {
            version: body.v,
            room: room_arr,
            psk: psk_arr,
            expires_at: body.exp,
        };
        ensure!(!invite.is_expired(now_secs), "convite expirado");
        Ok(invite)
    }
}

fn hex_tail(room: &[u8; ROOM_BYTES]) -> String {
    room[ROOM_BYTES - 2..]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_redaction_hide_secret() -> Result<()> {
        let invite = Invite::fixed_for_tests();
        let token = invite.encode();
        assert!(token.starts_with(INVITE_PREFIX));
        let parsed = Invite::decode(&token, 1_699_000_000)?;
        assert_eq!(parsed, invite);
        let redacted = invite.redacted();
        assert!(redacted.starts_with("rh1."));
        assert!(!redacted.contains(&URL_SAFE_NO_PAD.encode(invite.psk)));
        assert!(!token.is_empty());
        Ok(())
    }

    #[test]
    fn rejects_truncated_wrong_version_oversize_and_expired() {
        let invite = Invite::fixed_for_tests();
        let token = invite.encode();
        let truncated = &token[..token.len() - 4];
        assert!(Invite::decode(truncated, 1_699_000_000).is_err());
        assert!(Invite::decode("xx1.abc", 1_699_000_000).is_err());
        assert!(Invite::decode(&"a".repeat(MAX_INVITE_CHARS + 1), 1_699_000_000).is_err());
        assert!(Invite::decode(&token, 1_800_000_000).is_err());
    }
}
