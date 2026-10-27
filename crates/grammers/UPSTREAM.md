Source: https://codeberg.org/Lonami/grammers
Commit: 4ca2263e5678d2e85414aa8c2f469c1341d3a143
Retrieved: 2026-10-26
Licenses: LICENSE-MIT and LICENSE-APACHE (original files retained).

The GitHub mirror https://github.com/Lonami/grammers was archived; this copy comes from Codeberg.
Local changes:
- MemorySession::snapshot and Clone for SessionData allow durable, host-owned protobuf session storage.
- InputMessage::random_id accepts a host idempotency key; send_message passes it to Telegram.
- DialogIter::offset resumes a conversation cursor without replaying earlier pages.
- MTProto 2.0 key derivation calls the Boost-licensed td-rust port (grammers-crypto/DEPS.md documents it).
- The parent workspace consumes local path dependencies. SQLite storage is disabled.

The original copyright/license headers are retained in source files.

- Client::request_qr_login supports auth.exportLoginToken/importLoginToken, DC migration and the existing 2FA completion path.
