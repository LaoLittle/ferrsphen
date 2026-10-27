Source: https://github.com/tdlib/td
Commit: ea3f69ce11aef458bfbb09869b809ede4447486f
License: Boost Software License 1.0 (LICENSE_1_0.txt).

This is an incremental Rust port of the components listed below.

Ported components:
- td/mtproto/TcpTransport.cpp: intermediate TCP frame parsing, initialization tag and framing.
- td/mtproto/KDF.cpp: MTProto 2.0 key derivation.
- tdutils/td/utils/tl_storers.h and tl_parsers.h: bounded TL bytes/string encoding.

Rust adds explicit bounds, checked lengths and errors for malformed input. The Telegram adapter uses
this transport with the vendored pure Rust Grammers MTProto/session engine. TDLib managers, the TD
JSON API and secret-chat state machines have not been ported.
