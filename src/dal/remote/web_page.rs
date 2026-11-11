use crate::{
    core::config::{Config, FORBIDDEN_WEB_IP},
    domain::{
        models::{context::ContextText, events::Source, web_page::*},
        services::chunks::split,
    },
};
use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::sync::Semaphore;
use url::{Host, Url};
pub fn allowed_address(ip: IpAddr) -> bool {
    ip.to_canonical() != IpAddr::V4(FORBIDDEN_WEB_IP)
}
pub fn page_url(value: &str) -> Result<Url, PageFailure> {
    let mut url = Url::parse(value).map_err(|_| PageFailure::Unavailable)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(PageFailure::Unavailable);
    }
    let allowed = match url.host() {
        Some(Host::Ipv4(ip)) => allowed_address(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => allowed_address(IpAddr::V6(ip)),
        Some(Host::Domain(_)) => true,
        None => false,
    };
    if !allowed {
        return Err(PageFailure::Unavailable);
    }
    url.set_fragment(None);
    Ok(url)
}
struct WebResolver;
impl Resolve for WebResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let addresses: Vec<SocketAddr> = addresses
                .filter(|address| allowed_address(address.ip()))
                .collect();
            if addresses.is_empty() {
                return Err(std::io::Error::other("The web host is unavailable").into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}
#[derive(Clone)]
pub struct PageReader {
    client: reqwest::Client,
    slots: Arc<Semaphore>,
    config: Config,
}
impl PageReader {
    pub fn new(config: &Config) -> Result<Self, PageFailure> {
        let client = reqwest::Client::builder()
            .user_agent(&config.web_user_agent)
            .timeout(config.web_page_timeout)
            .no_proxy()
            .dns_resolver(Arc::new(WebResolver))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| PageFailure::Unavailable)?;
        Ok(Self {
            client,
            slots: Arc::new(Semaphore::new(config.web_parallel_pages.max(1))),
            config: config.clone(),
        })
    }
    pub async fn read(&self, value: &str) -> Result<WebPage, PageFailure> {
        let url = page_url(value)?;
        let _permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PageFailure::Unavailable)?;
        tokio::time::timeout(self.config.web_page_timeout, async {
            let response = self.client.get(url.clone()).send().await.map_err(|e| {
                if e.is_timeout() {
                    PageFailure::Timeout
                } else {
                    PageFailure::Unavailable
                }
            })?;
            if !response.status().is_success() {
                return Err(PageFailure::Unavailable);
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .unwrap_or_default()
                .trim();
            if !content_type.eq_ignore_ascii_case("text/html") {
                return Err(PageFailure::WrongType);
            }
            if response
                .content_length()
                .is_some_and(|n| n > self.config.web_html_bytes as u64)
            {
                return Err(PageFailure::TooLarge);
            }
            let mut bytes = vec![];
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    if e.is_timeout() {
                        PageFailure::Timeout
                    } else {
                        PageFailure::Unavailable
                    }
                })?;
                if chunk.len() > self.config.web_html_bytes.saturating_sub(bytes.len()) {
                    return Err(PageFailure::TooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            let html = String::from_utf8_lossy(&bytes).into_owned();
            let config = self.config.clone();
            tokio::task::spawn_blocking(move || extract(&config, url.as_str(), &html))
                .await
                .map_err(|_| PageFailure::NoMainText)?
        })
        .await
        .unwrap_or(Err(PageFailure::Timeout))
    }
}
pub fn extract(config: &Config, url: &str, html: &str) -> Result<WebPage, PageFailure> {
    let base = page_url(url)?;
    let mut readability = dom_smoothie::Readability::new(html, Some(url), None)
        .map_err(|_| PageFailure::NoMainText)?;
    let article = readability.parse().map_err(|_| PageFailure::NoMainText)?;
    if article.text_content.trim().is_empty() {
        return Err(PageFailure::NoMainText);
    }
    let chunks = split(config, &article.text_content)
        .into_iter()
        .map(|text| ContextText {
            label: article.title.clone(),
            source: Some(url.into()),
            text,
        })
        .collect();
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("a[href]").expect("fixed selector");
    let mut seen = HashSet::new();
    let mut links = vec![];
    for link in document.select(&selector) {
        if let Some(url) = link
            .attr("href")
            .and_then(|href| base.join(href).ok())
            .and_then(|url| page_url(url.as_str()).ok())
        {
            let url = String::from(url);
            if seen.insert(url.clone()) {
                links.push(WebLink {
                    url,
                    text: link
                        .text()
                        .collect::<String>()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                });
            }
        }
    }
    Ok(WebPage {
        source: Source {
            title: article.title,
            url: url.into(),
        },
        chunks,
        links,
    })
}

impl PageReader {
    /// One bounded robots GET, using the same connection policy and global slots.
    pub async fn robots(&self, target: &str) -> Result<(u16, Vec<u8>), PageFailure> {
        let url = page_url(target)?
            .join("/robots.txt")
            .map_err(|_| PageFailure::Unavailable)?;
        let _permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PageFailure::Unavailable)?;
        tokio::time::timeout(self.config.web_page_timeout, async {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|_| PageFailure::Unavailable)?;
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                return Ok((status, vec![]));
            }
            if response
                .content_length()
                .is_some_and(|n| n > self.config.web_robots_bytes as u64)
            {
                return Err(PageFailure::TooLarge);
            }
            let mut bytes = vec![];
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| PageFailure::Unavailable)?;
                if chunk.len() > self.config.web_robots_bytes.saturating_sub(bytes.len()) {
                    return Err(PageFailure::TooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok((status, bytes))
        })
        .await
        .unwrap_or(Err(PageFailure::Timeout))
    }
}
