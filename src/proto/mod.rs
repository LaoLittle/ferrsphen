include!(concat!(env!("OUT_DIR"), "/ferrsphen.storage.rs"));

mod codec;
pub(crate) use codec::{decode, encode, is_protobuf, transcode_legacy};

pub mod expressions {
    include!(concat!(env!("OUT_DIR"), "/ferrsphen.expressions.rs"));
}
