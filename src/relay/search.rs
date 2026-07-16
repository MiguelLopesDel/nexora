//! Web search backends for the relay. SearxNG is the recommended local
//! option; DuckDuckGo's HTML endpoint needs no key but may be rate-limited
//! or bot-challenged; Brave is a hosted API with a free tier.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::RelayConfig;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0";

#[derive(Debug, Clone)]
pub enum Backend {
    Searxng(String),
    DuckDuckGo,
    Brave(String),
    Off,
}

impl Backend {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Searxng(_) => "searxng",
            Self::DuckDuckGo => "duckduckgo",
            Self::Brave(_) => "brave",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub fn resolve(config: &RelayConfig) -> Result<Backend> {
    let brave_key = std::env::var(&config.brave_api_key_env).ok();
    let searxng = config.searxng_url.trim().trim_end_matches('/').to_string();
    match config.search.as_str() {
        "off" => Ok(Backend::Off),
        "searxng" => {
            if searxng.is_empty() {
                bail!("search = \"searxng\" requires searxng_url in [relay]");
            }
            Ok(Backend::Searxng(searxng))
        }
        "duckduckgo" => Ok(Backend::DuckDuckGo),
        "brave" => brave_key.map(Backend::Brave).context(format!(
            "search = \"brave\" requires the {} environment variable",
            config.brave_api_key_env
        )),
        "auto" => Ok(if !searxng.is_empty() {
            Backend::Searxng(searxng)
        } else if let Some(key) = brave_key {
            Backend::Brave(key)
        } else {
            Backend::DuckDuckGo
        }),
        other => bail!("unknown [relay] search backend `{other}`"),
    }
}

pub async fn search(backend: &Backend, query: &str, max_results: usize) -> Result<Vec<Hit>> {
    match backend {
        Backend::Searxng(base) => searxng(base, query, max_results).await,
        Backend::DuckDuckGo => duckduckgo(query, max_results).await,
        Backend::Brave(key) => brave(key, query, max_results).await,
        Backend::Off => bail!("web search is disabled in [relay]"),
    }
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .build()
        .context("could not build HTTP client")
}

async fn searxng(base: &str, query: &str, max_results: usize) -> Result<Vec<Hit>> {
    let response = client()?
        .get(format!("{base}/search"))
        .query(&[("q", query), ("format", "json")])
        .send()
        .await
        .context("searxng request failed")?
        .error_for_status()
        .context("searxng returned an error")?;
    let body: Value = response.json().await.context("searxng sent invalid JSON")?;
    let results = body["results"].as_array().cloned().unwrap_or_default();
    Ok(results
        .iter()
        .filter_map(|result| {
            Some(Hit {
                title: result["title"].as_str()?.to_string(),
                url: result["url"].as_str()?.to_string(),
                snippet: result["content"].as_str().unwrap_or_default().to_string(),
            })
        })
        .take(max_results)
        .collect())
}

async fn brave(key: &str, query: &str, max_results: usize) -> Result<Vec<Hit>> {
    let response = client()?
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query)])
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .send()
        .await
        .context("brave search request failed")?
        .error_for_status()
        .context("brave search returned an error")?;
    let body: Value = response.json().await.context("brave sent invalid JSON")?;
    let results = body["web"]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(results
        .iter()
        .filter_map(|result| {
            Some(Hit {
                title: result["title"].as_str()?.to_string(),
                url: result["url"].as_str()?.to_string(),
                snippet: result["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .take(max_results)
        .collect())
}

async fn duckduckgo(query: &str, max_results: usize) -> Result<Vec<Hit>> {
    let response = client()?
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .context("duckduckgo request failed")?
        .error_for_status()
        .context("duckduckgo returned an error")?;
    let html = response.text().await.context("duckduckgo sent no body")?;
    let hits = parse_duckduckgo(&html, max_results);
    if hits.is_empty() && html.contains("challenge") {
        bail!("duckduckgo is bot-challenging this network; configure searxng_url or Brave");
    }
    Ok(hits)
}

/// Extract results from DuckDuckGo's HTML endpoint without an HTML parser:
/// anchors classed `result__a` carry the link and title, `result__snippet`
/// the description. Links are indirected through /l/?uddg=<encoded-url>.
fn parse_duckduckgo(html: &str, max_results: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    for block in html.split("class=\"result__a\"").skip(1) {
        if hits.len() >= max_results {
            break;
        }
        let Some(href) = attribute(block, "href") else {
            continue;
        };
        let Some(title_end) = block.find("</a>") else {
            continue;
        };
        let title = strip_tags(&block[..title_end]);
        let title = title
            .rsplit_once('>')
            .map_or(title.as_str(), |(_, tail)| tail)
            .trim()
            .to_string();
        let snippet = block
            .split_once("result__snippet")
            .and_then(|(_, rest)| rest.split_once('>'))
            .and_then(|(_, rest)| rest.split_once("</a>").or_else(|| rest.split_once("</td>")))
            .map(|(inner, _)| strip_tags(inner))
            .unwrap_or_default();
        let url = decode_duckduckgo_href(&href);
        if url.is_empty() || title.is_empty() {
            continue;
        }
        hits.push(Hit {
            title: unescape(&title),
            url,
            snippet: unescape(snippet.trim()),
        });
    }
    hits
}

fn attribute(tag_rest: &str, name: &str) -> Option<String> {
    let start = tag_rest.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag_rest[start..].find('"')? + start;
    Some(tag_rest[start..end].to_string())
}

/// DuckDuckGo hrefs look like //duckduckgo.com/l/?uddg=<percent-encoded>&rut=…
fn decode_duckduckgo_href(href: &str) -> String {
    if let Some((_, rest)) = href.split_once("uddg=") {
        let encoded = rest.split('&').next().unwrap_or(rest);
        return percent_decode(encoded);
    }
    if href.starts_with("http") {
        return href.to_string();
    }
    String::new()
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Fetch a page and return readable text: scripts/styles dropped, tags
/// stripped, whitespace collapsed, truncated to `max_chars`.
pub async fn fetch_page_text(url: &str, max_chars: usize) -> Result<String> {
    let response = client()?
        .get(url)
        .send()
        .await
        .context("page request failed")?
        .error_for_status()
        .context("page returned an error")?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.is_empty() && !content_type.contains("html") && !content_type.contains("text")
    {
        bail!("not a text page ({content_type})");
    }
    let html = response.text().await.context("page sent no body")?;
    let text = strip_tags(&html);
    let text = unescape(&text);
    Ok(text.chars().take(max_chars).collect())
}

fn strip_tags(html: &str) -> String {
    let mut cleaned = html.to_string();
    for container in ["script", "style", "noscript", "svg"] {
        cleaned = remove_container(&cleaned, container);
    }
    let mut out = String::with_capacity(cleaned.len() / 2);
    let mut in_tag = false;
    for c in cleaned.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove `<name …>…</name>` blocks entirely (their text is not content).
fn remove_container(html: &str, name: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(start) = lower[pos..].find(&open).map(|offset| offset + pos) {
        out.push_str(&html[pos..start]);
        match lower[start..].find(&close).map(|offset| offset + start) {
            Some(end) => pos = end + close.len(),
            None => return out,
        }
    }
    out.push_str(&html[pos..]);
    out
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_results_are_extracted_and_decoded() {
        let html = r#"
        <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&amp;rut=abc">Example <b>Title</b></a>
        <a class="result__snippet" href="x">A short &amp; useful snippet</a>
        <a rel="nofollow" class="result__a" href="https://direct.example.org/">Direct</a>
        "#;
        let hits = parse_duckduckgo(html, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/page");
        assert!(hits[0].title.contains("Example"));
        assert!(hits[0].snippet.contains("short & useful"));
        assert_eq!(hits[1].url, "https://direct.example.org/");
    }

    #[test]
    fn tags_scripts_and_entities_are_stripped_from_pages() {
        let html =
            "<html><script>var x=1;</script><body><h1>Hi</h1><p>a&amp;b  c</p></body></html>";
        assert_eq!(unescape(&strip_tags(html)), "Hi a&b c");
    }

    #[test]
    fn percent_decoding_handles_utf8() {
        assert_eq!(percent_decode("caf%C3%A9+com+p%C3%A3o"), "café com pão");
    }
}
