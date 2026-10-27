// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![cfg(feature = "html")]

use std::cell::Cell;

use grammers_tl_types as tl;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use html5ever::{Attribute, local_name as tag};
use jiff::fmt::temporal::DateTimeParser;
use jiff::{Timestamp, tz};

use super::common::{
    MENTION_URL_PREFIX, Segment, after, before, inject_into_message, telegram_string_len,
    update_entity_len,
};

const CODE_LANG_PREFIX: &str = "language-";

/// Parse a message containing HTML tags into plain text and the list of formatting entities understood by Telegram.
///
/// This is not the same as the HTML understood by Telegram's HTTP Bot API.
pub fn parse_html_message(message: &str) -> (String, Vec<tl::enums::MessageEntity>) {
    struct Sink {
        text: Cell<String>,
        entities: Cell<Vec<tl::enums::MessageEntity>>,
        offset: Cell<i32>,
    }

    impl TokenSink for Sink {
        type Handle = ();

        fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
            let mut text = self.text.take();
            let mut entities = self.entities.take();
            let mut offset = self.offset.get();

            let length = 0;

            match token {
                Token::TagToken(Tag {
                    kind: TagKind::StartTag,
                    name,
                    self_closing: _,
                    attrs,
                    had_duplicate_attributes: _,
                }) => match name {
                    n if n == tag!("b") || n == tag!("strong") => {
                        entities.push(tl::types::MessageEntityBold { offset, length }.into());
                    }
                    n if n == tag!("i") || n == tag!("em") => {
                        entities.push(tl::types::MessageEntityItalic { offset, length }.into());
                    }
                    n if n == tag!("s") || n == tag!("del") => {
                        entities.push(tl::types::MessageEntityStrike { offset, length }.into());
                    }
                    tag!("u") => {
                        entities.push(tl::types::MessageEntityUnderline { offset, length }.into());
                    }
                    tag!("blockquote") => {
                        let collapsed = attrs.into_iter().any(|a| &a.name.local == "expandable");

                        entities.push(
                            tl::types::MessageEntityBlockquote {
                                offset,
                                length,
                                collapsed,
                            }
                            .into(),
                        );
                    }
                    tag!("details") => {
                        entities.push(tl::types::MessageEntitySpoiler { offset, length }.into());
                    }
                    tag!("code") => {
                        match entities.iter_mut().rev().next() {
                            // If the previous tag is an open `<pre>`, don't add `<code>`;
                            // we most likely want to indicate `class="language-foo"`.
                            Some(tl::enums::MessageEntity::Pre(e)) if e.length == 0 => {
                                e.language = attrs
                                    .into_iter()
                                    .find(|a| {
                                        a.name.local == tag!("class")
                                            && a.value.starts_with(CODE_LANG_PREFIX)
                                    })
                                    .map(|a| a.value[CODE_LANG_PREFIX.len()..].to_string())
                                    .unwrap_or_else(|| "".to_string());
                            }
                            _ => {
                                entities
                                    .push(tl::types::MessageEntityCode { offset, length }.into());
                            }
                        }
                    }
                    tag!("pre") => {
                        entities.push(
                            tl::types::MessageEntityPre {
                                offset,
                                length,
                                language: "".to_string(),
                            }
                            .into(),
                        );
                    }
                    tag!("a") => {
                        let url = attrs
                            .into_iter()
                            .find(|a| a.name.local == tag!("href"))
                            .map(|a| a.value.to_string())
                            .unwrap_or_else(|| "".to_string());

                        if url.starts_with(MENTION_URL_PREFIX) {
                            let user_id = url[MENTION_URL_PREFIX.len()..].parse::<i64>().unwrap();
                            entities.push(
                                tl::types::MessageEntityMentionName {
                                    offset,
                                    length,
                                    user_id,
                                }
                                .into(),
                            );
                        } else {
                            entities.push(
                                tl::types::MessageEntityTextUrl {
                                    offset,
                                    length,
                                    url,
                                }
                                .into(),
                            );
                        }
                    }
                    tag!("time") => {
                        if let Some(entity) = parse_time_tag(offset, length, &attrs) {
                            entities.push(entity.into());
                        }
                    }
                    _ => {}
                },
                Token::TagToken(Tag {
                    kind: TagKind::EndTag,
                    name,
                    self_closing: _,
                    attrs: _,
                    had_duplicate_attributes: _,
                }) => match name {
                    n if n == tag!("b") || n == tag!("strong") => {
                        update_entity_len!(Bold(offset) in entities);
                    }
                    n if n == tag!("i") || n == tag!("em") => {
                        update_entity_len!(Italic(offset) in entities);
                    }
                    n if n == tag!("s") || n == tag!("del") => {
                        update_entity_len!(Strike(offset) in entities);
                    }
                    tag!("u") => {
                        update_entity_len!(Underline(offset) in entities);
                    }
                    tag!("blockquote") => {
                        update_entity_len!(Blockquote(offset) in entities);
                    }
                    tag!("details") => {
                        update_entity_len!(Spoiler(offset) in entities);
                    }
                    tag!("code") => {
                        match entities.iter_mut().rev().next() {
                            // If the previous tag is an open `<pre>`, don't update `<code>` len;
                            // we most likely want to indicate `class="language-foo"`.
                            Some(tl::enums::MessageEntity::Pre(e)) if e.length == 0 => {}
                            _ => {
                                update_entity_len!(Code(offset) in entities);
                            }
                        }
                    }
                    tag!("pre") => {
                        update_entity_len!(Pre(offset) in entities);
                    }
                    tag!("a") => {
                        match entities.iter_mut().rev().next() {
                            // If the previous url is a mention, don't close with `</a>`;
                            Some(tl::enums::MessageEntity::MentionName(_)) => {
                                update_entity_len!(MentionName(offset) in entities);
                            }
                            _ => {
                                update_entity_len!(TextUrl(offset) in entities);
                            }
                        }
                    }
                    tag!("time") => {
                        update_entity_len!(FormattedDate(offset) in entities);
                    }
                    _ => {}
                },
                Token::CharacterTokens(string) => {
                    text.push_str(&string);
                    offset += telegram_string_len(&string);
                }
                _ => {}
            }

            self.text.replace(text);
            self.entities.replace(entities);
            self.offset.replace(offset);

            TokenSinkResult::Continue
        }
    }

    let mut input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(message).try_reinterpret().unwrap());

    let tok = Tokenizer::new(
        Sink {
            text: Cell::new(String::with_capacity(message.len())),
            entities: Cell::new(Vec::new()),
            offset: Cell::new(0),
        },
        Default::default(),
    );
    let _ = tok.feed(&mut input);
    tok.end();

    let Sink { text, entities, .. } = tok.sink;

    (text.take(), entities.take())
}

/// Combine plain text and the list of formatting entities understood by Telegram into HTML.
pub fn generate_html_message(message: &str, entities: &[tl::enums::MessageEntity]) -> String {
    use grammers_tl_types::enums::MessageEntity as ME;

    // Getting this wrong isn't the end of the world so the wildcard pattern is used
    // (but it would still be a shame for it to be wrong).
    let mut insertions = Vec::with_capacity(
        entities
            .iter()
            .map(|entity| match entity {
                ME::Bold(_) => 2,
                ME::Italic(_) => 2,
                ME::Code(_) => 2,
                ME::Pre(e) => {
                    if e.language.is_empty() {
                        2
                    } else {
                        4
                    }
                }
                ME::TextUrl(_) => 4,
                ME::MentionName(_) => 4,
                ME::Underline(_) => 2,
                ME::Strike(_) => 2,
                ME::Blockquote(_) => 2,
                ME::Spoiler(_) => 2,
                ME::FormattedDate(_) => 8,
                _ => 0,
            })
            .sum(),
    );

    entities
        .iter()
        .enumerate()
        .for_each(|(i, entity)| match entity {
            ME::Unknown(_) => {}
            ME::Mention(_) => {}
            ME::Hashtag(_) => {}
            ME::BotCommand(_) => {}
            ME::Url(_) => {}
            ME::Email(_) => {}
            ME::Bold(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<b>")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</b>")));
            }
            ME::Italic(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<i>")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</i>")));
            }
            ME::Code(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<code>")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</code>")));
            }
            ME::Pre(e) => {
                if e.language.is_empty() {
                    insertions.push((before(i, 0, e.offset), Segment::Fixed("<pre>")));
                    insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</pre>")));
                } else {
                    insertions.push((
                        before(i, 0, e.offset),
                        Segment::Fixed("<pre><code class=\"language-"),
                    ));
                    insertions.push((before(i, 1, e.offset), Segment::String(&e.language)));
                    insertions.push((before(i, 2, e.offset), Segment::Fixed("\">")));
                    insertions.push((
                        after(i, 0, e.offset + e.length),
                        Segment::Fixed("</code></pre>"),
                    ));
                }
            }
            ME::TextUrl(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<a href=\"")));
                insertions.push((before(i, 1, e.offset), Segment::String(&e.url)));
                insertions.push((before(i, 2, e.offset), Segment::Fixed("\">")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</a>")));
            }
            ME::MentionName(e) => {
                insertions.push((
                    before(i, 0, e.offset),
                    Segment::Fixed("<a href=\"tg://user?id="),
                ));
                insertions.push((before(i, 1, e.offset), Segment::Number(e.user_id)));
                insertions.push((before(i, 2, e.offset), Segment::Fixed("\">")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</a>")));
            }
            ME::InputMessageEntityMentionName(_) => {}
            ME::Phone(_) => {}
            ME::Cashtag(_) => {}
            ME::Underline(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<u>")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</u>")));
            }
            ME::Strike(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<del>")));
                insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</del>")));
            }
            ME::Blockquote(e) => {
                if e.collapsed {
                    insertions.push((
                        before(i, 0, e.offset),
                        Segment::Fixed("<blockquote expandable>"),
                    ));
                } else {
                    insertions.push((before(i, 0, e.offset), Segment::Fixed("<blockquote>")));
                }
                insertions.push((
                    after(i, 0, e.offset + e.length),
                    Segment::Fixed("</blockquote>"),
                ));
            }
            ME::BankCard(_) => {}
            ME::Spoiler(e) => {
                insertions.push((before(i, 0, e.offset), Segment::Fixed("<details>")));
                insertions.push((
                    after(i, 0, e.offset + e.length),
                    Segment::Fixed("</details>"),
                ));
            }
            ME::FormattedDate(e) => {
                if let Ok(timestamp) = Timestamp::from_second(e.date as i64) {
                    insertions.push((before(i, 0, e.offset), Segment::Fixed("<time datetime=\"")));
                    insertions.push((before(i, 1, e.offset), Segment::Timestamp(timestamp)));

                    let any_format = e.relative
                        || e.day_of_week
                        || e.short_date
                        || e.long_date
                        || e.short_time
                        || e.long_time;
                    if any_format {
                        insertions
                            .push((before(i, 2, e.offset), Segment::Fixed("\" data-format=\"")));
                        if e.relative {
                            insertions.push((before(i, 3, e.offset), Segment::Fixed("r")));
                        } else {
                            if e.day_of_week {
                                insertions.push((before(i, 3, e.offset), Segment::Fixed("w")));
                            }
                            if e.short_date {
                                insertions.push((before(i, 4, e.offset), Segment::Fixed("d")));
                            } else if e.long_date {
                                insertions.push((before(i, 4, e.offset), Segment::Fixed("D")));
                            }
                            if e.short_time {
                                insertions.push((before(i, 5, e.offset), Segment::Fixed("t")));
                            } else if e.long_time {
                                insertions.push((before(i, 5, e.offset), Segment::Fixed("T")));
                            }
                        }
                    }

                    insertions.push((before(i, 6, e.offset), Segment::Fixed("\">")));
                    insertions.push((after(i, 0, e.offset + e.length), Segment::Fixed("</time>")));
                }
            }
            ME::CustomEmoji(_) => {}
            ME::DiffInsert(_) => {}
            ME::DiffReplace(_) => {}
            ME::DiffDelete(_) => {}
            ME::TonAddress(_) => {}
        });

    inject_into_message(message, insertions)
}

/// Parse a regular `<time>` HTML tag into a Telegram message entity.
///
/// Returns `None` if the `datetime` attribute value cannot be parsed for any reason.
fn parse_time_tag(
    offset: i32,
    length: i32,
    attrs: &[Attribute],
) -> Option<tl::types::MessageEntityFormattedDate> {
    let dt_str = attrs
        .iter()
        .find(|a| a.name.local == tag!("datetime"))
        .map(|a| a.value.to_string());

    let format = attrs
        .iter()
        .find(|a| a.name.local == *"data-format")
        .map(|a| a.value.to_string())
        .unwrap_or_else(|| "".to_string());

    if let Some(dt_str) = dt_str
        && let Some(timestamp) = parse_timestamp(&dt_str)
    {
        let tg_ts: i32 = timestamp.as_second() as i32;

        Some(create_formatted_date(offset, length, tg_ts, &format))
    } else {
        None
    }
}

/// Create a message entity for a given Unix timestamp (`date`) and format string.
///
/// The format string will be parsed and set the relevant flags on the message entity.
/// Flags will be adjusted during parsing to avoid invalid format configurations.
///
/// The order of the format parsing is significant.
/// As an example, given the format string `"rDt"`, the following steps take place:
/// 1. All flags start out disabled.
/// 2. `r` enables `relative` and disables all other flags.
/// 3. `D` disables `relative` and `short_date`, and enables `long_date`.
/// 4. `t` disables `relative`, enables `short_time`, and disables `long_time`.
fn create_formatted_date(
    offset: i32,
    length: i32,
    date: i32,
    format: &str,
) -> tl::types::MessageEntityFormattedDate {
    let mut relative = false;
    let mut short_time = false;
    let mut long_time = false;
    let mut short_date = false;
    let mut long_date = false;
    let mut day_of_week = false;

    for char in format.chars() {
        match char {
            'r' => {
                relative = true;
                short_time = false;
                long_time = false;
                short_date = false;
                long_date = false;
                day_of_week = false;
            }
            't' => {
                relative = false;
                short_time = true;
                long_time = false;
            }
            'T' => {
                relative = false;
                short_time = false;
                long_time = true;
            }
            'd' => {
                relative = false;
                short_date = true;
                long_date = false;
            }
            'D' => {
                relative = false;
                short_date = false;
                long_date = true;
            }
            'w' => {
                relative = false;
                day_of_week = true;
            }
            _ => {}
        }
    }

    tl::types::MessageEntityFormattedDate {
        offset,
        length,
        date,
        relative,
        short_time,
        long_time,
        short_date,
        long_date,
        day_of_week,
    }
}

/// Parses a string into a `Timestamp`.
///
/// If the string does not have a time zone or offset, an attempt will be made to parse it as
/// a "civil" datetime and treat it as UTC.
///
/// If parsing fails, `None` is returned.
fn parse_timestamp(s: &str) -> Option<Timestamp> {
    static PARSER: DateTimeParser = DateTimeParser::new()
        .offset_conflict(tz::OffsetConflict::AlwaysTimeZone)
        .disambiguation(tz::Disambiguation::Compatible);

    if let Ok(zdt) = PARSER.parse_zoned(s) {
        return Some(zdt.timestamp());
    }

    if let Ok(ts) = PARSER.parse_timestamp(s) {
        return Some(ts);
    }

    if let Ok(cdt) = PARSER.parse_datetime(s)
        && let Ok(cdt_utc) = cdt.in_tz("UTC")
    {
        return Some(cdt_utc.timestamp());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_leading() {
        // Intentionally use different casing to make sure that is handled well
        let (text, entities) = parse_html_message("<B>Hello</b> world!");
        assert_eq!(text, "Hello world!");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityBold {
                    offset: 0,
                    length: 5
                }
                .into()
            ]
        );
    }

    #[test]
    fn parse_trailing() {
        let (text, entities) = parse_html_message("Hello <strong>world!</strong>");
        assert_eq!(text, "Hello world!");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityBold {
                    offset: 6,
                    length: 6
                }
                .into()
            ]
        );
    }

    #[test]
    fn parse_emoji() {
        let (text, entities) = parse_html_message("A <b>little 🦀</b> here");
        assert_eq!(text, "A little 🦀 here");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityBold {
                    offset: 2,
                    length: 9
                }
                .into()
            ]
        );
    }

    #[test]
    fn parse_all_entities() {
        let (text, entities) = parse_html_message(
            "Some <b>bold</b> (<strong>strong</strong>), <i>italics</i> \
            (<em>cursive</em>), inline <code>code</code>, a <pre>pre</pre> \
            block, a <a href=\"https://example.com\"><b>link</b></a>, \
            <details>spoilers</details> and <a href=\"tg://user?id=12345678\">mentions</a>",
        );

        assert_eq!(
            text,
            "Some bold (strong), italics (cursive), inline code, a pre block, a link, spoilers and mentions"
        );
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityBold {
                    offset: 5,
                    length: 4
                }
                .into(),
                tl::types::MessageEntityBold {
                    offset: 11,
                    length: 6
                }
                .into(),
                tl::types::MessageEntityItalic {
                    offset: 20,
                    length: 7
                }
                .into(),
                tl::types::MessageEntityItalic {
                    offset: 29,
                    length: 7
                }
                .into(),
                tl::types::MessageEntityCode {
                    offset: 46,
                    length: 4
                }
                .into(),
                tl::types::MessageEntityPre {
                    offset: 54,
                    length: 3,
                    language: "".to_string()
                }
                .into(),
                tl::types::MessageEntityTextUrl {
                    offset: 67,
                    length: 4,
                    url: "https://example.com".to_string()
                }
                .into(),
                tl::types::MessageEntityBold {
                    offset: 67,
                    length: 4,
                }
                .into(),
                tl::types::MessageEntitySpoiler {
                    offset: 73,
                    length: 8,
                }
                .into(),
                tl::types::MessageEntityMentionName {
                    offset: 86,
                    length: 8,
                    user_id: 12345678
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_pre_with_lang() {
        let (text, entities) = parse_html_message(
            "Some <pre>pre</pre>, <code>normal</code> and \
            <pre><code class=\"language-rust\">rusty</code></pre> code",
        );

        assert_eq!(text, "Some pre, normal and rusty code");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityPre {
                    offset: 5,
                    length: 3,
                    language: "".to_string()
                }
                .into(),
                tl::types::MessageEntityCode {
                    offset: 10,
                    length: 6,
                }
                .into(),
                tl::types::MessageEntityPre {
                    offset: 21,
                    length: 5,
                    language: "rust".to_string()
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_empty_pre_and_lang() {
        let (text, entities) = parse_html_message(
            "Some empty <pre></pre> and <code class=\"language-rust\">code</code>",
        );

        assert_eq!(text, "Some empty  and code");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityCode {
                    offset: 16,
                    length: 4,
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_link_no_href() {
        let (text, entities) = parse_html_message("Some <a>empty link</a>, it does nothing");

        assert_eq!(text, "Some empty link, it does nothing");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityTextUrl {
                    offset: 5,
                    length: 10,
                    url: "".to_string()
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_time_with_zone() {
        let (text, entities) = parse_html_message(
            "A <time datetime=\"2026-09-27 12:13:14[Europe/Stockholm]\">time</time>",
        );

        assert_eq!(text, "A time");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityFormattedDate {
                    offset: 2,
                    length: 4,
                    date: 1790503994,
                    relative: false,
                    short_time: false,
                    long_time: false,
                    short_date: false,
                    long_date: false,
                    day_of_week: false,
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_time_with_timestamp() {
        let (text, entities) =
            parse_html_message("A <time datetime=\"2026-09-27T12:13:14+02:00\">time</time>");

        assert_eq!(text, "A time");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityFormattedDate {
                    offset: 2,
                    length: 4,
                    date: 1790503994,
                    relative: false,
                    short_time: false,
                    long_time: false,
                    short_date: false,
                    long_date: false,
                    day_of_week: false,
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_time_with_invalid_date() {
        let (text, entities) = parse_html_message("A <time datetime=\"invalid\">time</time>");

        assert_eq!(text, "A time");
        assert_eq!(entities, vec![]);
    }

    #[test]
    fn parse_time_with_both_valid_and_invalid_date() {
        let (text, entities) = parse_html_message(
            "A <time datetime=\"2026-09-27T12:13:14+02:00\">valid</time> and <time datetime=\"invalid\">invalid</time>",
        );

        assert_eq!(text, "A valid and invalid");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityFormattedDate {
                    offset: 2,
                    length: 5,
                    date: 1790503994,
                    relative: false,
                    short_time: false,
                    long_time: false,
                    short_date: false,
                    long_date: false,
                    day_of_week: false,
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_nested_entities() {
        let (text, entities) = parse_html_message("Some <b>bold <i>both</b> italics</i>");
        assert_eq!(text, "Some bold both italics");
        assert_eq!(
            entities,
            vec![
                tl::types::MessageEntityBold {
                    offset: 5,
                    length: 9
                }
                .into(),
                tl::types::MessageEntityItalic {
                    offset: 10,
                    length: 12
                }
                .into(),
            ]
        );
    }

    #[test]
    fn parse_then_unparse() {
        let html = "Some <b>bold</b>, <i>italics</i> inline <code>code</code>, \
        a <pre>pre</pre> block <pre><code class=\"language-rs\">use rust;</code></pre>, \
        a <a href=\"https://example.com\"><b>link</b></a>, <details>spoilers</details> and \
        <a href=\"tg://user?id=12345678\">mentions</a>";
        let (text, entities) = parse_html_message(html);
        let generated = generate_html_message(&text, &entities);
        assert_eq!(generated, html);
    }

    #[test]
    fn parse_then_unparse_overlapping() {
        let markdown = "<i>a</i><a href=\"https://example.com\"><b>b</b></a><code>c</code>";
        let (text, entities) = parse_html_message(markdown);
        let generated = generate_html_message(&text, &entities);
        assert_eq!(generated, markdown);
    }

    #[test]
    fn parse_then_unparse_time() {
        let html = "Some <time datetime=\"2026-09-27T12:13:14+02:00\">time</time>";
        let (text, entities) = parse_html_message(html);
        let generated = generate_html_message(&text, &entities);
        let expected = "Some <time datetime=\"2026-09-27T10:13:14Z\">time</time>";
        assert_eq!(generated, expected);
    }

    #[test]
    fn parse_then_unparse_time_with_format() {
        let html =
            "Some <time datetime=\"2026-09-27T12:13:14+02:00\" data-format=\"wDt\">time</time>";
        let (text, entities) = parse_html_message(html);
        let generated = generate_html_message(&text, &entities);
        let expected =
            "Some <time datetime=\"2026-09-27T10:13:14Z\" data-format=\"wDt\">time</time>";
        assert_eq!(generated, expected);
    }
}
