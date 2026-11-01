//! Opaque, random identifiers.
//!
//! Every id starts with a two-letter prefix saying what it names, followed by
//! random characters:
//!
//! | kind | prefix | shape | example |
//! |---|---|---|---|
//! | device | `dv` | 4 digits + 4 lowercase letters | `dv7304kqzm` |
//! | user | `ur` | 4 digits | `ur7304` |
//!
//! Endpoints have no id of their own: an address is what a scan produces and
//! what the API addresses them by.
//!
//! Users are digits-only on purpose: there are only ever a handful of them, so
//! `10⁴` candidates is plenty, and a six-character id is easier to read off a
//! terminal than a ten-character one.
//!
//! Random rather than sequential, deliberately:
//!
//! * an id says nothing about the inventory — not how many devices exist, not
//!   in what order they appeared, not what the next one will be;
//! * nothing has to be kept in sync: no sequence counter in `meta`, no
//!   renumbering when a device is forgotten, no collision with a deleted id;
//! * uniqueness is enforced by the primary key, and the space
//!   (`10⁴ · 26⁴ ≈ 4.6 · 10⁹` for devices) makes a retry vanishingly rare.

use rand_core::{OsRng, RngCore};

/// What an id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Device,
    User,
}

impl Kind {
    /// The two letters every id of this kind starts with.
    pub const fn prefix(self) -> &'static str {
        match self {
            Kind::Device => "dv",
            Kind::User => "ur",
        }
    }

    /// How many random digits and lowercase letters follow the prefix.
    pub const fn shape(self) -> (usize, usize) {
        match self {
            Kind::Device => (4, 4),
            // A handful of users: four digits is 10 000 candidates, and keeping
            // the id short makes it easier to read off a terminal.
            Kind::User => (4, 0),
        }
    }

    /// The exact length of an id of this kind.
    pub const fn id_len(self) -> usize {
        let (digits, letters) = self.shape();
        2 + digits + letters
    }
}

/// A fresh id of the given kind — `dv7304kqzm`, `ur7304`.
pub fn new_id(kind: Kind) -> String {
    // A u32 per character: 2³² is not a multiple of 10 or 26, but the bias is
    // ~10⁻⁹ and not worth a rejection loop.
    let mut bytes = [0u8; 4 * 8];
    OsRng.fill_bytes(&mut bytes);

    let (digits, letters) = kind.shape();
    let mut id = String::with_capacity(kind.id_len());
    id.push_str(kind.prefix());

    for (slot, chunk) in bytes.chunks_exact(4).take(digits + letters).enumerate() {
        let value = u32::from_le_bytes(chunk.try_into().expect("chunks_exact(4)"));
        id.push(if slot < digits {
            char::from(b'0' + (value % 10) as u8)
        } else {
            char::from(b'a' + (value % 26) as u8)
        });
    }

    id
}

/// True when `id` is shaped like an id of `kind`.
pub fn is_kind(id: &str, kind: Kind) -> bool {
    let Some(rest) = id.strip_prefix(kind.prefix()) else {
        return false;
    };
    let (digits, letters) = kind.shape();
    rest.len() == digits + letters
        && rest[..digits].bytes().all(|b| b.is_ascii_digit())
        && rest[digits..].bytes().all(|b| b.is_ascii_lowercase())
}

/// An id for a device.
pub fn device_id() -> String {
    new_id(Kind::Device)
}

/// An id for a user.
pub fn user_id() -> String {
    new_id(Kind::User)
}

/// A session token: 32 random bytes, URL-safe, no padding, no prefix.
///
/// Entropy matters here more than shape: 256 bits is beyond guessing, and
/// unlike ids there is no prefix — a token is never shaped like anything else.
pub fn session_token() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(kind: Kind) -> String {
        let id = new_id(kind);
        assert_eq!(id.len(), kind.id_len(), "{id}");
        assert!(is_kind(&id, kind), "{id}");
        id
    }

    #[test]
    fn ids_have_the_expected_shape() {
        assert!(fresh(Kind::Device).starts_with("dv"));
        assert!(fresh(Kind::User).starts_with("ur"));
    }

    #[test]
    fn devices_have_letters_and_users_do_not() {
        let device = device_id();
        assert_eq!(device.len(), 10);
        assert!(device[6..].bytes().all(|b| b.is_ascii_lowercase()));

        let user = user_id();
        assert_eq!(user.len(), 6, "{user}");
        assert!(user[2..].bytes().all(|b| b.is_ascii_digit()), "{user}");
    }

    #[test]
    fn the_prefix_says_what_the_id_names() {
        let device = device_id();
        assert!(is_kind(&device, Kind::Device));
        assert!(!is_kind(&device, Kind::User));

        let user = user_id();
        assert!(is_kind(&user, Kind::User));
        assert!(!is_kind(&user, Kind::Device));
    }

    #[test]
    fn fresh_ids_are_unique() {
        // Since there is no counter to keep in sync, uniqueness across calls is
        // the property that matters. (Uniqueness against *stored* ids is the
        // primary key's job; `inventory`/`db` cover that.)
        let ids: Vec<String> = (0..64).map(|_| device_id()).collect();
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            ids.len(),
            "fresh ids collided: {ids:?}"
        );
    }

    #[test]
    fn the_digit_and_letter_halves_are_populated() {
        // Guard against the whole space collapsing onto one character.
        let mut digits = std::collections::HashSet::new();
        let mut letters = std::collections::HashSet::new();
        for _ in 0..40 {
            let id = new_id(Kind::Device);
            digits.extend(id[2..6].chars());
            letters.extend(id[6..].chars());
        }
        assert!(digits.len() > 4, "digits look stuck: {digits:?}");
        assert!(letters.len() > 10, "letters look stuck: {letters:?}");
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad in [
            "",
            "dv",
            "dv12345678",  // letters where digits belong
            "DV1234abcd",  // uppercase prefix
            "dv1234ABCD",  // uppercase letters
            "dv12345abc",  // one digit too many
            "dv123abcd",   // one digit too few
            "ur1234abcd",  // users have digits only
            "ur123",       // one digit too few
            "dv1234 abcd", // stray space
        ] {
            assert!(!is_kind(bad, Kind::Device), "{bad}");
            assert!(!is_kind(bad, Kind::User), "{bad}");
        }
    }
}
