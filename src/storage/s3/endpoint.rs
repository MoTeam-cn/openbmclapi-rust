//! Storage endpoint URL parsing and object-key path arithmetic.

use url::Url;

use crate::error::{Error, Result};

/// Connection parameters for one S3-compatible endpoint.
#[derive(Debug, Clone)]
pub(super) struct Endpoint {
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) port: Option<u16>,
    pub(super) access_key: String,
    pub(super) secret_key: String,
    pub(super) region: String,
}

impl Endpoint {
    /// Parse a storage URL: credentials in the userinfo, `region` in the query.
    pub(super) fn parse(raw: &str, default_region: Option<&str>) -> Result<Self> {
        let url =
            Url::parse(raw).map_err(|e| Error::Config(format!("存储地址无效 {raw:?}：{e}")))?;
        let host = match url.host() {
            Some(url::Host::Ipv6(addr)) => format!("[{addr}]"),
            Some(url::Host::Ipv4(addr)) => addr.to_string(),
            Some(url::Host::Domain(domain)) => domain.to_string(),
            None => return Err(Error::Config(format!("存储地址 {raw:?} 没有主机名"))),
        };
        let region = url
            .query_pairs()
            .find(|(key, _)| key.as_ref() == "region")
            .map(|(_, value)| value.into_owned())
            .or_else(|| default_region.map(str::to_string))
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok(Endpoint {
            scheme: url.scheme().to_string(),
            host,
            port: url.port(),
            access_key: url.username().to_string(),
            secret_key: url.password().unwrap_or_default().to_string(),
            region,
        })
    }

    /// `host[:port]`, matching the HTTP `Host` header.
    pub(super) fn authority(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{}", self.host, port),
            None => self.host.clone(),
        }
    }

    /// `scheme://host[:port]`.
    pub(super) fn base_url(&self) -> String {
        format!("{}://{}", self.scheme, self.authority())
    }
}

/// Split `<bucket>/<prefix...>` out of a storage URL path.
pub(super) fn split_bucket_prefix(raw: &str) -> Result<(String, String)> {
    let url = Url::parse(raw).map_err(|e| Error::Config(format!("存储地址无效 {raw:?}：{e}")))?;
    let mut segments = url.path().split('/').filter(|segment| !segment.is_empty());
    let bucket = segments
        .next()
        .ok_or_else(|| Error::Config(format!("存储地址 {raw:?} 没有 bucket")))?
        .to_string();
    let prefix = segments.collect::<Vec<_>>().join("/");
    Ok((bucket, prefix))
}

#[cfg(test)]
#[path = "endpoint_test.rs"]
mod tests;
