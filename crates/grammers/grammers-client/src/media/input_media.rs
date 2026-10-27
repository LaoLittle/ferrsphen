// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::time::Duration;

use grammers_tl_types as tl;

use super::{Attribute, Media, Uploaded};

/// Media item to be sent as part of an album.
#[derive(Default)]
pub struct InputMedia {
    pub(crate) entities: Vec<tl::enums::MessageEntity>,
    pub(crate) reply_to: Option<i32>,
    pub(crate) caption: String,
    pub(crate) media: Option<tl::enums::InputMedia>,
    media_ttl: Option<i32>,
    mime_type: Option<String>,
}

/// Convenience builder for an audio [`InputMedia`].
///
/// # Examples
///
/// ```
/// # async fn f(client: &mut grammers_client::Client) -> Result<(), Box<dyn std::error::Error>> {
/// use std::time::Duration;
/// use grammers_client::media::InputAudio;
/// use grammers_client::message::InputMessage;
///
/// let uploaded = client.upload_file("target/input.mp3").await?;
/// let thumb = client.upload_file("target/cover.mp3").await?;
/// let media = InputAudio::new(uploaded)
///     .mime_type("audio/mpeg")
///     .thumb(Some(thumb))
///     .duration(Duration::from_secs(128))
///     .title(Some("SongName"))
///     .performer(Some("SongAuthor"));
/// let media = InputMessage::new().media(media);
/// # Ok(())
/// # }
pub struct InputAudio {
    file: Uploaded,
    force_file: bool,
    mime_type: String,
    thumb: Option<Uploaded>,
    voice: bool,
    duration: i32,
    title: Option<String>,
    performer: Option<String>,
    waveform: Option<Vec<u8>>,
}

/// Convenience builder for an video [`InputMedia`].
///
/// # Examples
///
/// ```
/// # async fn f(client: &mut grammers_client::Client) -> Result<(), Box<dyn std::error::Error>> {
/// use std::time::Duration;
/// use grammers_client::media::InputVideo;
/// use grammers_client::message::InputMessage;
///
/// let uploaded = client.upload_file("target/input.mp4").await?;
/// let thumb = client.upload_file("target/first_frame.jpg").await?;
/// let media = InputVideo::new(uploaded)
///     .mime_type("video/mp4")
///     .thumb(Some(thumb))
///     .duration(Duration::from_secs_f64(13.78))
///     .size(1920, 1080);
/// let media = InputMessage::new().media(media);
/// # Ok(())
/// # }
pub struct InputVideo {
    file: Uploaded,
    force_file: bool,
    spoiler: bool,
    nosound: bool,
    nosound_video: bool,
    animated: bool,
    mime_type: String,
    thumb: Option<Uploaded>,
    video_cover: Option<tl::enums::InputPhoto>,
    video_timestamp: Option<i32>,
    ttl: Option<i32>,
    stickers: Option<Vec<tl::enums::InputDocument>>,
    round_message: bool,
    supports_streaming: bool,
    duration: f64,
    w: i32,
    h: i32,
    preload_prefix_size: Option<i32>,
    video_start_ts: Option<f64>,
    video_codec: Option<String>,
}

/// Media's Time To Live (TTL).
#[derive(Clone, Debug)]
pub enum TimeToLive {
    /// Never self-destruct.
    Unlimited,
    /// Immediately self-destruct after it's viewed.
    ViewableOnce,
    /// Viewable for the specified duration once opened.
    ///
    /// Note that some specific durations are reserved by Telegram
    /// and have special meanings, while others might be too long.
    ///
    /// Durations outside these limits will be clamped to be within them.
    Duration(Duration),
}

impl From<TimeToLive> for Option<i32> {
    fn from(x: TimeToLive) -> Option<i32> {
        match x {
            TimeToLive::Unlimited => None,
            TimeToLive::ViewableOnce => Some(i32::MAX),
            TimeToLive::Duration(d) => Some(
                d.as_secs()
                    .try_into()
                    .unwrap_or(i32::MAX)
                    .clamp(1, i32::MAX - 1),
            ),
        }
    }
}

impl InputMedia {
    /// Creates a new empty media message for input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the plaintext in the message.
    ///
    /// The caller must ensure that formatting entities remain valid for the given text.
    /// If you need to update formatting entities, call method [`InputMedia::fmt_entities`].
    ///
    /// <div class="warning">
    /// Note that this method does not modify formatting entities, which may break
    /// formatting or cause out-of-bounds errors if entities do not match the given text.
    /// </div>
    pub fn caption<T>(mut self, s: T) -> Self
    where
        T: Into<String>,
    {
        self.caption = s.into();
        self
    }

    /// The formatting entities within the caption (such as bold, italics, etc.).
    pub fn fmt_entities<I>(mut self, entities: I) -> Self
    where
        I: IntoIterator<Item = tl::enums::MessageEntity>,
    {
        self.entities = entities.into_iter().collect();
        self
    }

    /// Builds a new media from the given markdown-formatted string as the
    /// caption contents and entities.
    ///
    /// Note that Telegram only supports a very limited subset of entities:
    /// bold, italic, underline, strikethrough, code blocks, pre blocks and inline links (inline
    /// links with this format `tg://user?id=12345678` will be replaced with inline mentions when
    /// possible).
    #[cfg(feature = "markdown")]
    pub fn markdown<T>(mut self, s: T) -> Self
    where
        T: AsRef<str>,
    {
        let (caption, entities) = crate::parsers::parse_markdown_message(s.as_ref());
        self.caption = caption;
        self.entities = entities;
        self
    }

    /// Builds a new media from the given HTML-formatted string as the
    /// caption contents and entities.
    ///
    /// Note that Telegram only supports a very limited subset of entities:
    /// bold, italic, underline, strikethrough, code blocks, pre blocks and inline links (inline
    /// links with this format `tg://user?id=12345678` will be replaced with inline mentions when
    /// possible).
    #[cfg(feature = "html")]
    pub fn html<T>(mut self, s: T) -> Self
    where
        T: AsRef<str>,
    {
        let (caption, entities) = crate::parsers::parse_html_message(s.as_ref());
        self.caption = caption;
        self.entities = entities;
        self
    }

    /// The album identifier to which this album should reply to, if any.
    ///
    /// Otherwise, this album will not be a reply to any other.
    ///
    /// Only the reply_to from the first media is used.
    pub fn reply_to(mut self, reply_to: Option<i32>) -> Self {
        self.reply_to = reply_to;
        self
    }

    /// Include the uploaded file as a photo in the album.
    ///
    /// The Telegram server will compress the image and convert it to JPEG format if necessary.
    ///
    /// The text will be the caption of the photo, which may be empty for no caption.
    pub fn photo(mut self, file: Uploaded) -> Self {
        self.media = Some(
            (tl::types::InputMediaUploadedPhoto {
                spoiler: false,
                file: file.raw,
                stickers: None,
                ttl_seconds: self.media_ttl,
                live_photo: false,
                video: None,
            })
            .into(),
        );
        self
    }

    /// Include an external photo in the album.
    ///
    /// The Telegram server will download and compress the image and convert it to JPEG format if
    /// necessary.
    ///
    /// The text will be the caption of the photo, which may be empty for no caption.
    pub fn photo_url(mut self, url: impl Into<String>) -> Self {
        self.media = Some(
            (tl::types::InputMediaPhotoExternal {
                spoiler: false,
                url: url.into(),
                ttl_seconds: self.media_ttl,
            })
            .into(),
        );
        self
    }

    /// Include the uploaded file as a document in the album.
    ///
    /// You can use this to send videos, stickers, audios, or uncompressed photos.
    ///
    /// The text will be the caption of the document, which may be empty for no caption.
    pub fn document(mut self, file: Uploaded) -> Self {
        let mime_type = self.get_file_mime(&file);
        self.media = Some(super::uploaded_document(
            file,
            mime_type,
            false,
            self.media_ttl,
        ));
        self
    }

    /// Include a media in the message using the raw TL types.
    ///
    /// You can use this to send any media using the raw TL types that don't have
    /// a specific method in this builder such as Dice, Polls, etc.
    ///
    /// This can also be used to send media with a file reference, see `InputMediaDocument`
    /// and `InputMediaPhoto` in the `grammers-tl-types` crate.
    ///
    /// The text will be the caption of the media, which may be empty for no caption.
    pub fn media<M: Into<tl::enums::InputMedia>>(mut self, media: M) -> Self {
        self.media = Some(media.into());
        self
    }

    /// Include the video file with thumb in the album.
    ///
    /// The text will be the caption of the document, which may be empty for no caption.
    ///
    /// # Examples
    ///
    /// ```
    /// async fn f(client: &mut grammers_client::Client) -> Result<(), Box<dyn std::error::Error>> {
    ///     use grammers_client::media::InputMedia;
    ///
    ///     let video = client.upload_file("video.mp4").await?;
    ///     let thumb = client.upload_file("thumb.png").await?;
    ///     let media = InputMedia::new().caption("").document(video).thumbnail(thumb);
    ///     Ok(())
    /// }
    /// ```
    pub fn thumbnail(mut self, thumb: Uploaded) -> Self {
        if let Some(tl::enums::InputMedia::UploadedDocument(document)) = &mut self.media {
            document.thumb = Some(thumb.raw);
        }
        self
    }

    /// Include an external file as a document in the album.
    ///
    /// You can use this to send videos, stickers, audios, or uncompressed photos.
    ///
    /// The Telegram server will be the one that downloads and includes the document as media.
    ///
    /// The text will be the caption of the document, which may be empty for no caption.
    pub fn document_url(mut self, url: impl Into<String>) -> Self {
        self.media = Some(
            (tl::types::InputMediaDocumentExternal {
                spoiler: false,
                url: url.into(),
                ttl_seconds: self.media_ttl,
                video_cover: None,
                video_timestamp: None,
            })
            .into(),
        );
        self
    }

    /// Add additional attributes to the media.
    ///
    /// This must be called *after* setting a file.
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn f(client: &mut grammers_client::Client) -> Result<(), Box<dyn std::error::Error>> {
    /// # let audio = client.upload_file("audio.flac").await?;
    /// #
    /// use std::time::Duration;
    /// use grammers_client::media::{Attribute, InputMedia};
    ///
    /// let media = InputMedia::new().caption("").document(audio).attribute(
    ///    Attribute::Audio {
    ///        duration: Duration::new(123, 0),
    ///        title: Some("Hello".to_string()),
    ///        performer: Some("World".to_string()),
    ///    }
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn attribute(mut self, attr: Attribute) -> Self {
        if let Some(tl::enums::InputMedia::UploadedDocument(document)) = &mut self.media {
            document.attributes.push(attr.into());
        }
        self
    }

    /// Copy media from an existing message.
    ///
    /// You can use this to send media from another message without re-uploading it.
    pub fn copy_media(mut self, media: &Media) -> Self {
        self.media = media.to_raw_input_media();
        self
    }

    /// Include the uploaded file as a document file in the album.
    ///
    /// You can use this to send any type of media as a simple document file.
    ///
    /// The text will be the caption of the file, which may be empty for no caption.
    pub fn file(mut self, file: Uploaded) -> Self {
        let mime_type = self.get_file_mime(&file);
        self.media = Some(super::uploaded_document(
            file,
            mime_type,
            true,
            self.media_ttl,
        ));
        self
    }

    /// Change the media's Time To Live (TTL).
    ///
    /// For example, this enables you to send a `photo` that can only be viewed for a certain
    /// amount of seconds before it expires.
    ///
    /// Not all media supports this feature.
    ///
    /// This method should be called before setting any media, else it won't have any effect.
    pub fn media_ttl(mut self, ttl: TimeToLive) -> Self {
        self.media_ttl = ttl.into();
        self
    }

    /// Change the media's mime type.
    ///
    /// This method will override the mime type that would otherwise be automatically inferred
    /// from the extension of the used file
    ///
    /// If no mime type is set and it cannot be inferred, the mime type will be
    /// "application/octet-stream".
    ///
    /// This method should be called before setting any media, else it won't have any effect.
    pub fn mime_type(mut self, mime_type: &str) -> Self {
        self.mime_type = Some(mime_type.to_string());
        self
    }

    /// Return the mime type string for the given file.
    fn get_file_mime(&self, file: &Uploaded) -> String {
        if let Some(mime) = self.mime_type.as_ref() {
            mime.clone()
        } else if let Some(mime) = mime_guess::from_path(file.name()).first() {
            mime.essence_str().to_string()
        } else {
            "application/octet-stream".to_string()
        }
    }
}

impl InputAudio {
    pub fn new(uploaded: Uploaded) -> Self {
        Self {
            file: uploaded,
            force_file: false,
            mime_type: String::new(),
            thumb: None,
            voice: false,
            duration: 0,
            title: None,
            performer: None,
            waveform: None,
        }
    }

    /// Sets whether force the media file to be uploaded as document.
    pub fn force_file(mut self, force_file: bool) -> Self {
        self.force_file = force_file;
        self
    }

    /// Sets the mime type of the video.
    ///
    /// Note: if the mime_type is not audio, it will not be displayed as audio.
    pub fn mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = mime_type.into();
        self
    }

    /// Sets the thumbnail of the audio.
    pub fn thumb(mut self, thumb: Option<Uploaded>) -> Self {
        self.thumb = thumb;
        self
    }

    /// Sets whether this is a voice message.
    pub fn voice(mut self, voice: bool) -> Self {
        self.voice = voice;
        self
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration.as_secs().try_into().unwrap();
        self
    }

    /// Sets the name of the song.
    pub fn title(mut self, title: Option<impl Into<String>>) -> Self {
        self.title = title.map(Into::into);
        self
    }

    /// Sets the performer of the song.
    pub fn performer(mut self, performer: Option<impl Into<String>>) -> Self {
        self.performer = performer.map(Into::into);
        self
    }

    /// Sets the waveform of the song.
    pub fn waveform(mut self, waveform: Option<Vec<u8>>) -> Self {
        self.waveform = waveform;
        self
    }
}

impl From<InputAudio> for tl::enums::InputMedia {
    fn from(x: InputAudio) -> Self {
        let InputAudio {
            file,
            force_file,
            mime_type,
            thumb,
            voice,
            duration,
            title,
            performer,
            waveform,
        } = x;
        let file_name = file.name().to_string();
        tl::types::InputMediaUploadedDocument {
            nosound_video: false,
            force_file,
            spoiler: false,
            file: file.raw,
            thumb: thumb.map(|t| t.raw),
            mime_type,
            attributes: vec![
                tl::types::DocumentAttributeFilename { file_name }.into(),
                tl::types::DocumentAttributeAudio {
                    voice,
                    duration,
                    title,
                    performer,
                    waveform,
                }
                .into(),
            ],
            stickers: None,
            ttl_seconds: None,
            video_cover: None,
            video_timestamp: None,
        }
        .into()
    }
}

impl InputVideo {
    pub fn new(uploaded: Uploaded) -> Self {
        Self {
            file: uploaded,
            force_file: false,
            spoiler: false,
            nosound: false,
            nosound_video: false,
            animated: false,
            mime_type: String::new(),
            thumb: None,
            video_cover: None,
            video_timestamp: None,
            ttl: None,
            stickers: None,
            round_message: false,
            supports_streaming: true,
            duration: 0.0,
            w: 0,
            h: 0,
            preload_prefix_size: None,
            video_start_ts: None,
            video_codec: None,
        }
    }

    /// Sets whether force the media file to be uploaded as document.
    pub fn force_file(mut self, force_file: bool) -> Self {
        self.force_file = force_file;
        self
    }

    /// Sets whether this video should be hidden behind a spoiler warning.
    pub fn spoiler(mut self, spoiler: bool) -> Self {
        self.spoiler = spoiler;
        self
    }

    /// Sets whether this video does not contain an audio track.
    pub fn nosound(mut self, nosound: bool) -> Self {
        self.nosound = nosound;
        self
    }

    /// Sets whether to keep videos without an audio track as videos.
    ///
    /// * If `true` (default), such videos are still sent as videos.
    /// * If `false`, such videos are sent as animations (`DocumentAttributeAnimated`) instead.
    pub fn nosound_video(mut self, nosound_video: bool) -> Self {
        self.nosound_video = nosound_video;
        self
    }

    /// Sets whether the video is displayed as an animated GIF.
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self.nosound_video = false;
        self
    }

    /// Sets the mime type of the video.
    ///
    /// Note: if the mime_type is not video, it will not be displayed as video.
    pub fn mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = mime_type.into();
        self
    }

    /// Sets the thumbnail of the video.
    pub fn thumb(mut self, thumb: Option<Uploaded>) -> Self {
        self.thumb = thumb;
        self
    }

    /// Sets the cover of the video.
    ///
    /// tip: [`tl::enums::InputPhoto`] can get from [`super::Photo::to_raw_input_photo`].
    pub fn video_cover(mut self, video_cover: Option<tl::enums::InputPhoto>) -> Self {
        self.video_cover = video_cover;
        self
    }

    /// Sets the timestamp at which to start playing the video.
    pub fn video_timestamp(mut self, video_timestamp: Option<Duration>) -> Self {
        self.video_timestamp = video_timestamp.map(|t| t.as_secs().try_into().unwrap());
        self
    }

    /// Change the video's Time To Live (TTL).
    ///
    /// This enables you to send a `video` that can only be viewed for a certain
    /// amount of seconds before it expires.
    pub fn ttl(mut self, ttl: TimeToLive) -> Self {
        self.ttl = ttl.into();
        self
    }

    /// Attached stickers.
    pub fn stickers(mut self, stickers: Option<Vec<tl::enums::InputDocument>>) -> Self {
        self.stickers = stickers;
        self
    }

    /// Sets whether this is a round video.
    pub fn round_message(mut self, round_message: bool) -> Self {
        self.round_message = round_message;
        self
    }

    /// Sets whether the video supports streaming
    pub fn supports_streaming(mut self, supports_streaming: bool) -> Self {
        self.supports_streaming = supports_streaming;
        self
    }

    /// Sets the duration of the video.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration.as_secs_f64();
        self
    }

    // Sets the width and height of the video.
    pub fn size(mut self, w: i32, h: i32) -> Self {
        self.w = w;
        self.h = h;
        self
    }

    /// Number of bytes to preload when preloading videos.
    pub fn preload_prefix_size(mut self, preload_prefix_size: Option<i32>) -> Self {
        self.preload_prefix_size = preload_prefix_size;
        self
    }

    /// Floating point UNIX timestamp in seconds, indicating the frame of the video that
    /// should be used as static preview and thumbnail.
    pub fn video_start_ts(mut self, video_start_ts: Option<Duration>) -> Self {
        self.video_start_ts = video_start_ts.map(|d| d.as_secs_f64());
        self
    }

    /// Sets the codec used for the video, i.e. "h264", "h265", or "av1".
    pub fn video_codec(mut self, video_codec: Option<impl Into<String>>) -> Self {
        self.video_codec = video_codec.map(Into::into);
        self
    }
}

impl From<InputVideo> for tl::enums::InputMedia {
    fn from(x: InputVideo) -> Self {
        let InputVideo {
            file,
            force_file,
            spoiler,
            nosound,
            nosound_video,
            animated,
            mime_type,
            thumb,
            video_cover,
            video_timestamp,
            ttl,
            stickers,
            round_message,
            supports_streaming,
            duration,
            w,
            h,
            preload_prefix_size,
            video_start_ts,
            video_codec,
        } = x;
        let file_name = file.name().to_string();
        let mut attributes = vec![
            tl::types::DocumentAttributeFilename { file_name }.into(),
            tl::types::DocumentAttributeVideo {
                round_message,
                supports_streaming,
                nosound,
                duration,
                w,
                h,
                preload_prefix_size,
                video_start_ts,
                video_codec,
            }
            .into(),
        ];
        if animated {
            attributes.push(tl::types::DocumentAttributeAnimated {}.into())
        }

        tl::types::InputMediaUploadedDocument {
            nosound_video,
            force_file,
            spoiler,
            file: file.raw,
            thumb: thumb.map(|t| t.raw),
            mime_type,
            attributes,
            stickers,
            ttl_seconds: ttl,
            video_cover,
            video_timestamp,
        }
        .into()
    }
}
