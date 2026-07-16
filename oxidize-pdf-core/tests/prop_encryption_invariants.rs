//! Encryption invariants, stated from the contract of "encrypt then decrypt is
//! the identity", not from a bug.
//!
//! Invariants (for RC4-40, RC4-128, AES-128, AES-256):
//!   1. ROUND-TRIP — a document encrypted with a user/owner password, written,
//!      re-read, and unlocked with the correct password recovers its content
//!      verbatim. Guards the "written encrypted document reads back empty/garbage"
//!      class (#364), across arbitrary content and passwords including an empty
//!      user password (#379).
//!   2. OWNER PASSWORD — the owner password also unlocks and recovers content.
//!   3. WRONG PASSWORD — a password matching neither user nor owner never unlocks
//!      (fail-safe: no silent open).

use oxidize_pdf::document::{DocumentEncryption, EncryptionStrength};
use oxidize_pdf::encryption::Permissions;
use oxidize_pdf::parser::PdfReader;
use oxidize_pdf::text::ExtractionOptions;
use oxidize_pdf::{Document, Font, Page};
use proptest::prelude::*;
use std::io::Cursor;

fn build_encrypted(
    content: &str,
    user: &str,
    owner: &str,
    strength: EncryptionStrength,
) -> Vec<u8> {
    let mut doc = Document::new();
    let mut page = Page::new(595.0, 842.0);
    page.text()
        .set_font(Font::Helvetica, 24.0)
        .at(72.0, 760.0)
        .write(content)
        .unwrap();
    doc.add_page(page);
    doc.set_encryption(DocumentEncryption::new(
        user,
        owner,
        Permissions::all(),
        strength,
    ));
    doc.to_bytes().expect("write encrypted document")
}

/// Unlock with `password` and extract page 0. `None` = the password did not
/// unlock the document (or it was not encrypted as expected).
fn unlock_and_extract(bytes: &[u8], password: &str) -> Option<String> {
    let mut reader = PdfReader::new(Cursor::new(bytes.to_vec())).expect("parse written PDF");
    assert!(reader.is_encrypted(), "written document must be encrypted");
    if !reader.unlock_with_password(password).expect("unlock call") {
        return None;
    }
    let doc = reader.into_document();
    Some(
        doc.extract_text_from_page_with_options(0, ExtractionOptions::default())
            .expect("extract text")
            .text,
    )
}

fn strengths() -> impl Strategy<Value = EncryptionStrength> {
    prop::sample::select(vec![
        EncryptionStrength::Rc4_40bit,
        EncryptionStrength::Rc4_128bit,
        EncryptionStrength::Aes128,
        EncryptionStrength::Aes256,
    ])
}

// Printable, non-space ASCII marker that survives a single WinAnsi write() and
// extracts as one contiguous run.
fn marker() -> impl Strategy<Value = String> {
    "[A-Za-z0-9]{4,24}"
}

// Passwords: alphanumeric plus the PDF string delimiters `(`, `)`, `\` and a
// space. User may be empty (#379); owner is non-empty (the standard case). The
// delimiters are included on purpose: they exercise the `/O`/`/U` bytes that
// #430 and the owner-unlock fail-open both mishandled, so keeping them in the
// generator guards that both stay fixed.
fn user_pw() -> impl Strategy<Value = String> {
    r"[A-Za-z0-9()\\ ]{0,16}"
}
fn owner_pw() -> impl Strategy<Value = String> {
    r"[A-Za-z0-9()\\ ]{1,16}"
}

/// Guard for #430: a `(` in the user password used to break owner-password
/// unlock. Root cause was shared with the owner-unlock fail-open below — the old
/// owner path truncated the decrypted `/O` bytes at the first `0x28` ('('), so a
/// user password of `(` collapsed to `""` and the correct owner password no
/// longer matched. Authenticating the raw 32 padded bytes (the fix) resolves
/// both. Formerly `#[ignore]`d as blocked-on-#430; now a permanent guard.
#[test]
fn issue_430_paren_in_user_password_owner_unlock() {
    for strength in [
        EncryptionStrength::Rc4_40bit,
        EncryptionStrength::Rc4_128bit,
        EncryptionStrength::Aes128,
    ] {
        let bytes = build_encrypted("MARKER", "(", "owner", strength);
        let mut reader = PdfReader::new(Cursor::new(bytes)).expect("parse");
        assert!(
            reader.unlock_with_password("owner").expect("unlock call"),
            "owner password must unlock when the user password contains '(' ({strength:?}, #430)"
        );
    }
}

/// Deterministic guard for the owner-unlock fail-open: a wrong password must not
/// authenticate as owner. The R2-R4 owner path decrypts `/O` to the padded user
/// password, then (before the fix) searched for where the standard padding began
/// (`0x28`, `(`) and truncated there. With a wrong password the 32 decrypted
/// bytes are garbage; whenever the first byte happened to be `0x28` the derived
/// password collapsed to `""`, which re-padded to the exact standard padding and
/// so authenticated any document with an **empty** user password — granting
/// owner-level access on ~1/256 of wrong attempts. The fix runs the decrypted
/// bytes through the standard user-auth (ISO 32000-1 §7.6.3.4, Alg. 3) with no
/// truncation. This input is a concrete wrong password that opened before it.
#[test]
fn wrong_owner_password_never_grants_access_with_empty_user_pw() {
    let owner = "ladLlQaGYZ";
    // Wrong password whose `/O` decryption yields a `0x28` first byte (the shrunk
    // proptest counterexample). Differs from both "" (user) and owner.
    let wrong = format!("\u{1}wrong\u{2}{owner}");
    for strength in [
        EncryptionStrength::Rc4_40bit,
        EncryptionStrength::Rc4_128bit,
        EncryptionStrength::Aes128,
    ] {
        let bytes = build_encrypted("000a", "", owner, strength);
        let mut reader = PdfReader::new(Cursor::new(bytes)).expect("parse");
        assert!(reader.is_encrypted());
        assert!(
            !reader.unlock_with_password(&wrong).expect("unlock call"),
            "a wrong password must not unlock an empty-user-password doc ({strength:?})"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// The correct user password recovers the content, for every strength.
    #[test]
    fn user_password_round_trips(
        content in marker(),
        user in user_pw(),
        owner in owner_pw(),
        strength in strengths(),
    ) {
        let bytes = build_encrypted(&content, &user, &owner, strength);
        let extracted = unlock_and_extract(&bytes, &user);
        prop_assert!(
            extracted.as_deref().map(|t| t.contains(&content)).unwrap_or(false),
            "user password failed to recover content ({strength:?}); got {:?}",
            extracted
        );
    }

    /// The owner password also recovers the content.
    #[test]
    fn owner_password_round_trips(
        content in marker(),
        user in user_pw(),
        owner in owner_pw(),
        strength in strengths(),
    ) {
        let bytes = build_encrypted(&content, &user, &owner, strength);
        let extracted = unlock_and_extract(&bytes, &owner);
        prop_assert!(
            extracted.as_deref().map(|t| t.contains(&content)).unwrap_or(false),
            "owner password failed to recover content ({strength:?}); got {:?}",
            extracted
        );
    }

    /// A password matching neither user nor owner never unlocks.
    #[test]
    fn wrong_password_never_unlocks(
        content in marker(),
        user in user_pw(),
        owner in owner_pw(),
        strength in strengths(),
    ) {
        // Control chars can't appear in the generated passwords, so this differs
        // from both user and owner by construction.
        let wrong = format!("{user}\u{1}wrong\u{2}{owner}");
        let bytes = build_encrypted(&content, &user, &owner, strength);
        let mut reader = PdfReader::new(Cursor::new(bytes)).expect("parse");
        prop_assert!(reader.is_encrypted());
        let unlocked = reader.unlock_with_password(&wrong).expect("unlock call");
        prop_assert!(!unlocked, "a wrong password must not unlock ({strength:?})");
    }
}
