# gramme.rs

A set of Rust crates to interact with Telegram's API, hence the name *(tele)gramme.rs*.

## Current status

It works! The high-level interface is slowly taking shape, and it can already be used to [build
real projects], such as [RSS bots].

For an up-to-date taste on how the library looks like, refer to the [client examples] folder.

For the API reference, please refer to <https://docs.rs/grammers-client/>.

## Crates

![Diagram depicting the crate hierarchy](assets/crate-hierarchy.svg)

* **[grammers-client]**: high-level API. Depends on:
  * `grammers-tl-types` to both [invoke requests] and wrap [raw types].
  * `grammers-session` to persist home [datacenter], logged-in user and [cache peers].
  * `grammers-mtsender` to connect to Telegram's servers and exchange messages.
  * `grammers-crypto` to support [Two-Factor Authentication] logins.
* **[grammers-mtsender]**: network connection to Telegram. Depends on:
  * `grammers-tl-types` to initialize the connection and offer an ergonomic [RPC interface].
  * `grammers-session` to persist the DC configuration and corresponding [Authorization Key]s.
  * `grammers-mtproto` to serialize messages and manage the connection state.
  * `grammers-crypto` for efficient buffer usage.
* **[grammers-session]**: session storages for the client. Depends on:
  * `grammers-tl-types` to provide a more ergonomic interface over peers.
* **[grammers-mtproto]**: implementation of the [Mobile Transport Protocol]. Depends on:
  * `grammers-tl-types` to invoke and parse the core messages of the protocol.
  * `grammers-crypto` to encrypt the communication with Telegram.
* **[grammers-tl-types]**: generated Rust types for a certain layer. Depends on:
  * `grammers-tl-gen` to generate the Rust code that makes up the crate itself.
  * `grammers-tl-parser` to parse the TL files with all definitions used by Telegram.
* **[grammers-crypto]**: cryptography-related methods.
* **[grammers-tl-gen]**: Rust code generator from TL definitions. Depends on:
  * `grammers-tl-parser` to compose functions referencing the parsed definition types.
* **[grammers-tl-parser]**: a [Type Language] parser.

## Security

It is recommended to always use [cargo-crev] to verify the trustworthiness of each of your
dependencies, including this one.

As far as I know, this code has not been audited, so if, for any reason, you're using this crate
where security is critical, I strongly encourage you to review at least `grammers-crypto` and the
authentication part of `grammers-mtproto`. I am not a security expert, although I trust my code
enough to use it myself.

If you know about some published audit for this crate, please let me know, so that I can link it
here and review the issues found.

## License

All the libraries and binaries contained in this repository are licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE] or
  http://www.apache.org/licenses/LICENSE-2.0)

* MIT license ([LICENSE-MIT] or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Thank you for considering to contribute! I'll try my best to provide quick, constructive feedback
on your issues or pull requests. Please do call me out if you think my behaviour is not acceptable
at any time. I will try to keep the discussion as technical as possible. Similarly, I will not
tolerate poor behaviour from your side towards other people (including myself).

**Please only submit issues and pull requests authored entirely by you or other fellow humans.**
If you don't feel capable of authoring code yourself, I encourage you to still try, as I believe
that's the best way to learn and get better. If you're still not comfortable, please submit a
feature-request instead, for discussion, so that someone else can implement it instead.

You may use translation assistance if writing English is also not something you're comfortable with.
But please choose a translation tool that doesn't overly try to rewrite and twist your thoughts.
Browsers like Firefox have a decently good offline translator built-in that you may use.

It is recommended to run these commands to make the Git experience a bit nicer:

```sh
git config --local blame.ignoreRevsFile .git-blame-ignore-revs
cp pre-commit .git/hooks/
```

If you don't have the time to [contribute code], you may contribute by [reporting issues] or
feature ideas. Please note that every feature added will increase maintenance burden on my part,
so be mindful when suggesting things. It may be possible that your idea could exist as its own
crate, offered as [extensions to grammers].

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

[build real projects]: https://codeberg.org/Lonami/grammers/wiki/Real-world-projects
[RSS bots]: https://codeberg.org/Lonami/srsrssrsbot
[client examples]: grammers-client/examples
[Mobile Transport Protocol]: https://core.telegram.org/mtproto
[Type Language]: https://core.telegram.org/mtproto/TL
[grammers-client]: grammers-client/
[grammers-crypto]: grammers-crypto/
[grammers-mtproto]: grammers-mtproto/
[grammers-mtsender]: grammers-mtsender/
[grammers-session]: grammers-session/
[grammers-tl-gen]: grammers-tl-gen/
[grammers-tl-parser]: grammers-tl-parser/
[grammers-tl-types]: grammers-tl-types/
[invoke requests]: https://core.telegram.org/methods
[raw types]: https://core.telegram.org/schema
[datacenter]: https://core.telegram.org/api/datacenter
[cache peers]: https://core.telegram.org/api/peers
[Two-Factor Authentication]: https://core.telegram.org/api/srp
[RPC interface]: https://core.telegram.org/api/invoking
[Authorization Key]: https://core.telegram.org/mtproto/auth_key
[scrape-docs]: bin/scrape-docs/
[tl-to-json]: bin/tl-to-json/
[tl-json]: https://core.telegram.org/schema/json
[cargo-crev]: https://crates.io/crates/cargo-crev
[LICENSE-APACHE]: LICENSE-APACHE
[LICENSE-MIT]: LICENSE-MIT
[contribute code]: https://codeberg.org/Lonami/srsrssrsbot/pulls
[reporting issues]: https://codeberg.org/Lonami/srsrssrsbot/issues/new
[extensions to grammers]: https://codeberg.org/Lonami/grammers/wiki/Client-extensions
