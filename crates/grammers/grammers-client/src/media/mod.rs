// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Types relating to message media and downloadables.
//!
//! Properties containing raw types are public and will either be called "raw" or prefixed with "raw_".\
//! Keep in mind that **these fields are not part of the semantic versioning guarantees**.

mod attributes;
mod downloadable;
mod input_media;
#[allow(clippy::module_inception)]
mod media;
mod photo_sizes;

pub use attributes::Attribute;
pub use downloadable::Downloadable;
use grammers_tl_types as tl;
pub use input_media::{InputAudio, InputMedia, InputVideo, TimeToLive};
pub use media::{
    ChatPhoto, Contact, Dice, Document, Geo, GeoLive, Media, Photo, Poll, Sticker, Uploaded, Venue,
    WebPage,
};
pub use photo_sizes::PhotoSize;

/// Build an `InputMediaUploadedDocument` from an uploaded file.
///
/// Shared by the `document` (`force_file = false`) and `file` (`force_file = true`)
/// builder methods of `InputMedia` and `InputMessage`.
pub(crate) fn uploaded_document(
    file: Uploaded,
    mime_type: String,
    force_file: bool,
    ttl_seconds: Option<i32>,
) -> tl::enums::InputMedia {
    let file_name = file.name().to_string();
    tl::types::InputMediaUploadedDocument {
        nosound_video: false,
        force_file,
        spoiler: false,
        file: file.raw,
        thumb: None,
        mime_type,
        attributes: vec![(tl::types::DocumentAttributeFilename { file_name }).into()],
        stickers: None,
        ttl_seconds,
        video_cover: None,
        video_timestamp: None,
    }
    .into()
}
