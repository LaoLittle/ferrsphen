// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Rich message that can hold additional formatting options.

use grammers_tl_types as tl;

/// Construct a rich message.
#[derive(Clone)]
pub struct InputRichMessage {
    pub raw: tl::enums::InputRichMessage,
}

impl InputRichMessage {
    /// Constructs a rich message from raw PageBlocks.
    pub fn blocks(blocks: Vec<tl::enums::PageBlock>) -> Self {
        Self {
            raw: tl::types::InputRichMessage {
                rtl: false,
                noautolink: false,
                blocks,
                photos: None,
                documents: None,
                users: None,
            }
            .into(),
        }
    }

    /// Constructs a rich message from an HTML string.
    ///
    /// For details on the supported HTML tags and formatting, please refer to the
    /// [Telegram Bot API documentation](https://core.telegram.org/bots/api#rich-html-style).
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_client::message::InputRichMessage;
    /// let rich_message = InputRichMessage::html("<b>Hello</b> <i>World!</i>");
    /// ```
    pub fn html<T>(html: T) -> Self
    where
        T: Into<String>,
    {
        Self {
            raw: tl::types::InputRichMessageHtml {
                rtl: false,
                noautolink: false,
                html: html.into(),
                files: None,
            }
            .into(),
        }
    }

    /// Constructs a rich message from a Markdown string.
    ///
    /// For details on the supported Markdown formatting, please refer to the
    /// [Telegram Bot API documentation](https://core.telegram.org/bots/api#rich-markdown-style).
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_client::message::InputRichMessage;
    /// let rich_message = InputRichMessage::markdown("**Hello** _World!_");
    /// ```
    pub fn markdown<T>(markdown: T) -> Self
    where
        T: Into<String>,
    {
        Self {
            raw: tl::types::InputRichMessageMarkdown {
                rtl: false,
                noautolink: false,
                markdown: markdown.into(),
                files: None,
            }
            .into(),
        }
    }

    /// Enables right-to-left (RTL) text direction for this rich message.
    pub fn rtl(mut self) -> Self {
        match self.raw {
            tl::enums::InputRichMessage::Message(ref mut r) => {
                r.rtl = true;
            }
            tl::enums::InputRichMessage::Html(ref mut r) => {
                r.rtl = true;
            }
            tl::enums::InputRichMessage::Markdown(ref mut r) => {
                r.rtl = true;
            }
        }
        self
    }

    pub fn noautolink(mut self) -> Self {
        match self.raw {
            tl::enums::InputRichMessage::Message(ref mut r) => {
                r.noautolink = true;
            }
            tl::enums::InputRichMessage::Html(ref mut r) => {
                r.noautolink = true;
            }
            tl::enums::InputRichMessage::Markdown(ref mut r) => {
                r.noautolink = true;
            }
        }
        self
    }

    /// Attaches an [`tl::enums::InputPhoto`] to the rich message.
    /// (You can get it by [`crate::media::Photo::to_raw_input_photo`].)
    ///
    /// When using HTML or Markdown formatting, photos are embedded by referencing a custom
    /// URI scheme: `tg://photo?id={id}`. This method binds the corresponding `id` string
    /// to the provided photo.
    ///
    /// For blocks-based rich messages ([`InputRichMessage::blocks`]), the `id` can be left empty
    /// because it uses i64 to reference photo.
    ///
    /// # Examples
    ///
    /// ```
    /// use grammers_client::message::InputRichMessage;
    /// use grammers_tl_types as tl;
    ///
    /// let photo: tl::enums::InputPhoto = tl::types::InputPhoto {
    ///     id: 5087011400000000000,
    ///     access_hash: 1186849320000000000,
    ///     file_reference: vec![1, 0, 0, 0, 82, 106],
    /// }
    /// .into();
    ///
    /// // 1. Using raw PageBlocks
    /// let rich_message = InputRichMessage::blocks(vec![
    ///     tl::types::PageBlockPhoto {
    ///         spoiler: false,
    ///         photo_id: 5087011415335308568,
    ///         caption: tl::types::PageCaption {
    ///             text: tl::types::TextPlain {
    ///                 text: "Photo caption".to_string()
    ///             }.into(),
    ///             credit: tl::types::TextPlain {
    ///                 text: "Photo credit".to_string()
    ///             }.into(),
    ///         }.into(),
    ///         url: None,
    ///         webpage_id: None,
    ///     }
    ///     .into(),
    /// ])
    /// .photo("", photo.clone());
    ///
    /// // 2. Using HTML format
    /// let rich_message = InputRichMessage::html(
    ///     r#"<figure>
    ///     <img src="tg://photo?id=photo_id" tg-spoiler/>
    ///         <figcaption>Photo caption<cite>Photo credit</cite></figcaption>
    ///     </figure>"#,
    /// )
    /// .photo("photo_id", photo.clone());
    ///
    /// // 3. Using Markdown format
    /// let rich_message = InputRichMessage::markdown("![](tg://photo?id=photo_id \"Photo caption\")")
    ///     .photo("photo_id", photo);
    /// ```
    pub fn photo<T>(mut self, id: T, photo: tl::enums::InputPhoto) -> Self
    where
        T: Into<String>,
    {
        match self.raw {
            tl::enums::InputRichMessage::Message(ref mut r) => {
                r.photos.get_or_insert_with(Vec::new).push(photo);
            }
            tl::enums::InputRichMessage::Html(ref mut r) => {
                r.files.get_or_insert_with(Vec::new).push(
                    tl::types::InputRichFilePhoto {
                        id: id.into(),
                        photo,
                    }
                    .into(),
                )
            }
            tl::enums::InputRichMessage::Markdown(ref mut r) => {
                r.files.get_or_insert_with(Vec::new).push(
                    tl::types::InputRichFilePhoto {
                        id: id.into(),
                        photo,
                    }
                    .into(),
                )
            }
        }
        self
    }

    /// Attaches an [`tl::enums::InputDocument`] to the rich message.
    /// (You can get it by [`crate::media::Document::to_raw_input_document`].)
    ///
    /// When using HTML or Markdown formatting, documents are embedded by referencing a custom
    /// URI scheme: `tg://video?id={id}` or `tg://audio?id={id}` or `tg://document?id={id}`.
    /// This method binds the corresponding `id` string to the provided document.
    ///
    /// For blocks-based rich messages ([`InputRichMessage::blocks`]), the `id` can be left empty
    /// because it uses i64 to reference video/audio/document.
    pub fn document<T>(mut self, id: T, document: tl::enums::InputDocument) -> Self
    where
        T: Into<String>,
    {
        match self.raw {
            tl::enums::InputRichMessage::Message(ref mut r) => {
                r.documents.get_or_insert_with(Vec::new).push(document);
            }
            tl::enums::InputRichMessage::Html(ref mut r) => {
                r.files.get_or_insert_with(Vec::new).push(
                    tl::types::InputRichFileDocument {
                        id: id.into(),
                        document,
                    }
                    .into(),
                )
            }
            tl::enums::InputRichMessage::Markdown(ref mut r) => {
                r.files.get_or_insert_with(Vec::new).push(
                    tl::types::InputRichFileDocument {
                        id: id.into(),
                        document,
                    }
                    .into(),
                )
            }
        }
        self
    }
}
