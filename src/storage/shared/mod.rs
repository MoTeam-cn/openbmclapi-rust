//! Helpers shared by the S3/MinIO and Aliyun OSS backends.

mod http;
mod keys;
mod xml;

pub(crate) use http::{copy_passthrough, ensure_success};
pub(crate) use keys::{
    encode_component, encode_filename, encode_path, join_object_key, strip_prefix_key,
};
pub(crate) use xml::{xml_blocks, xml_tag, ListedObject};
