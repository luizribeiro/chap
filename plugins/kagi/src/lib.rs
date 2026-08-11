mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "tool-plugin",
        metadata: {
            id: "kagi",
            name: "Kagi web tools",
            version: "0.1.0",
            description: "Searches the web and extracts readable page content with Kagi",
        },
    });
}

use bindings::exports::sage::agent::tools::{Guest, ToolDefinition};
use bindings::sage::agent::settings;
use http::{HeaderMap, HeaderValue, header};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use url::Url;
use wasi_fetch::Client;

const API_BASE_URL: &str = "https://kagi.com/api/v1";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const WEB_FETCH: &str = "web_fetch";
const WEB_SEARCH: &str = "web_search";
const DEFAULT_SEARCH_LIMIT: usize = 10;
const MAX_SEARCH_LIMIT: usize = 20;
const DEFAULT_MAX_CHARS: usize = 30_000;
const MIN_MAX_CHARS: usize = 1_000;
const MAX_MAX_CHARS: usize = 100_000;

struct Kagi;

impl Guest for Kagi {
    async fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Ok(tool_definitions())
    }

    async fn execute(name: String, arguments: String) -> Result<String, String> {
        match name.as_str() {
            WEB_SEARCH => {
                let arguments: SearchArguments = parse_arguments(WEB_SEARCH, &arguments)?;
                arguments.validate()?;
                let settings = load_settings().await?;
                let body = post_json(
                    "/search",
                    &settings.api_key,
                    &SearchRequest {
                        query: &arguments.query,
                        workflow: "search",
                        format: "json",
                        limit: arguments.limit,
                    },
                )
                .await?;
                search_output(&body, arguments.limit)
            }
            WEB_FETCH => {
                let arguments: FetchArguments = parse_arguments(WEB_FETCH, &arguments)?;
                arguments.validate()?;
                let settings = load_settings().await?;
                let body = post_json(
                    "/extract",
                    &settings.api_key,
                    &ExtractRequest {
                        pages: arguments.urls.iter().map(|url| PageInput { url }).collect(),
                        format: "json",
                    },
                )
                .await?;
                extract_output(&body, arguments.max_chars_per_page)
            }
            _ => Err(format!("tool `{name}` is not provided by the Kagi plugin")),
        }
    }
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: WEB_SEARCH.to_owned(),
            description: "Search the web with Kagi and return ranked titles, URLs, snippets, and publication dates."
                .to_owned(),
            parameters: r#"{
                "type":"object",
                "properties":{
                    "query":{"type":"string","description":"The web search query."},
                    "limit":{"type":"integer","minimum":1,"maximum":20,"default":10,"description":"Maximum number of results to return."}
                },
                "required":["query"],
                "additionalProperties":false
            }"#.to_owned(),
        },
        ToolDefinition {
            name: WEB_FETCH.to_owned(),
            description: "Extract clean Markdown content from up to 10 HTTPS web pages with Kagi.".to_owned(),
            parameters: r#"{
                "type":"object",
                "properties":{
                    "urls":{"type":"array","items":{"type":"string","format":"uri"},"minItems":1,"maxItems":10,"description":"HTTPS URLs to extract."},
                    "max_chars_per_page":{"type":"integer","minimum":1000,"maximum":100000,"default":30000,"description":"Maximum characters returned for each page."}
                },
                "required":["urls"],
                "additionalProperties":false
            }"#.to_owned(),
        },
    ]
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(tool: &str, arguments: &str) -> Result<T, String> {
    serde_json::from_str(arguments).map_err(|error| format!("invalid `{tool}` arguments: {error}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Settings {
    api_key: String,
}

impl Settings {
    fn from_json(json: &str) -> Result<Self, String> {
        let settings: Self = serde_json::from_str(json)
            .map_err(|error| format!("invalid Kagi settings: {error}"))?;
        if settings.api_key.trim().is_empty() {
            return Err("Kagi setting `api-key` is required".to_owned());
        }
        Ok(settings)
    }
}

async fn load_settings() -> Result<Settings, String> {
    let json = settings::get_json().await?;
    Settings::from_json(&json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

impl SearchArguments {
    fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("web search query cannot be empty".to_owned());
        }
        if !(1..=MAX_SEARCH_LIMIT).contains(&self.limit) {
            return Err(format!(
                "web search limit must be between 1 and {MAX_SEARCH_LIMIT}"
            ));
        }
        Ok(())
    }
}

const fn default_search_limit() -> usize {
    DEFAULT_SEARCH_LIMIT
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchArguments {
    urls: Vec<String>,
    #[serde(default = "default_max_chars")]
    max_chars_per_page: usize,
}

impl FetchArguments {
    fn validate(&self) -> Result<(), String> {
        if !(1..=10).contains(&self.urls.len()) {
            return Err("web fetch requires between 1 and 10 URLs".to_owned());
        }
        for value in &self.urls {
            let url =
                Url::parse(value).map_err(|error| format!("invalid URL `{value}`: {error}"))?;
            if url.scheme() != "https" || url.host_str().is_none() {
                return Err(format!("web fetch URL must be HTTPS: `{value}`"));
            }
        }
        if !(MIN_MAX_CHARS..=MAX_MAX_CHARS).contains(&self.max_chars_per_page) {
            return Err(format!(
                "max_chars_per_page must be between {MIN_MAX_CHARS} and {MAX_MAX_CHARS}"
            ));
        }
        Ok(())
    }
}

const fn default_max_chars() -> usize {
    DEFAULT_MAX_CHARS
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    workflow: &'static str,
    format: &'static str,
    limit: usize,
}

#[derive(Serialize)]
struct ExtractRequest<'a> {
    pages: Vec<PageInput<'a>>,
    format: &'static str,
}

#[derive(Serialize)]
struct PageInput<'a> {
    url: &'a str,
}

async fn post_json(path: &str, token: &str, body: &impl Serialize) -> Result<String, String> {
    let body = serde_json::to_vec(body)
        .map_err(|error| format!("failed to encode Kagi request: {error}"))?;
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::try_from(format!("Bearer {token}"))
            .map_err(|_| "Kagi API key contains invalid header characters".to_owned())?,
    );

    let url = format!("{API_BASE_URL}{path}");
    let response = Client::new()
        .post(&url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|error| format!("Kagi HTTP request failed: {error}"))?;
    let status = response.status().as_u16();
    let body = collect_limited(response.into_body(), MAX_RESPONSE_BYTES).await?;
    let body = String::from_utf8(body)
        .map_err(|error| format!("Kagi response was not valid UTF-8: {error}"))?;
    if !(200..300).contains(&status) {
        return Err(api_error(status, &body));
    }
    Ok(body)
}

async fn collect_limited(mut body: wasi_fetch::Body, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| format!("failed to read Kagi response: {error}"))?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(format!("Kagi response exceeded the {limit}-byte limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct SearchResponse {
    data: SearchData,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(default)]
    search: Vec<ApiSearchResult>,
}

#[derive(Deserialize)]
struct ApiSearchResult {
    url: String,
    title: String,
    snippet: Option<String>,
    time: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
struct SearchOutput {
    results: Vec<SearchResult>,
}

#[derive(Serialize, Debug, PartialEq)]
struct SearchResult {
    url: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published: Option<String>,
}

fn search_output(body: &str, limit: usize) -> Result<String, String> {
    let response: SearchResponse = serde_json::from_str(body)
        .map_err(|error| format!("invalid Kagi search response: {error}"))?;
    let output = SearchOutput {
        results: response
            .data
            .search
            .into_iter()
            .take(limit)
            .map(|result| SearchResult {
                url: result.url,
                title: decode_entities(result.title),
                snippet: result.snippet.map(decode_entities),
                published: result.time,
            })
            .collect(),
    };
    serde_json::to_string(&output)
        .map_err(|error| format!("failed to encode web search results: {error}"))
}

fn decode_entities(value: String) -> String {
    html_escape::decode_html_entities(&value).into_owned()
}

#[derive(Deserialize)]
struct ExtractResponse {
    data: Vec<ApiExtractedPage>,
}

#[derive(Deserialize)]
struct ApiExtractedPage {
    url: String,
    markdown: Option<String>,
    error: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
struct ExtractOutput {
    pages: Vec<ExtractedPage>,
}

#[derive(Serialize, Debug, PartialEq)]
struct ExtractedPage {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

fn extract_output(body: &str, max_chars: usize) -> Result<String, String> {
    let response: ExtractResponse = serde_json::from_str(body)
        .map_err(|error| format!("invalid Kagi extract response: {error}"))?;
    let output = ExtractOutput {
        pages: response
            .data
            .into_iter()
            .map(|page| {
                let (markdown, truncated) = match page.markdown {
                    Some(markdown) => {
                        let (markdown, truncated) = truncate(markdown, max_chars);
                        (Some(markdown), truncated)
                    }
                    None => (None, false),
                };
                ExtractedPage {
                    url: page.url,
                    markdown,
                    truncated,
                    error: page.error,
                }
            })
            .collect(),
    };
    serde_json::to_string(&output)
        .map_err(|error| format!("failed to encode extracted web pages: {error}"))
}

fn truncate(mut value: String, max_chars: usize) -> (String, bool) {
    let Some((index, _)) = value.char_indices().nth(max_chars) else {
        return (value, false);
    };
    value.truncate(index);
    (value, true)
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: Vec<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: String,
    message: Option<String>,
}

fn api_error(status: u16, body: &str) -> String {
    let details = serde_json::from_str::<ErrorResponse>(body)
        .ok()
        .map(|response| {
            response
                .error
                .into_iter()
                .map(|error| error.message.unwrap_or(error.code))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|details| !details.is_empty())
        .unwrap_or_else(|| body.chars().take(500).collect());
    format!("Kagi API returned HTTP {status}: {details}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_typed_settings() {
        let settings = Settings::from_json(r#"{"api-key":"key"}"#).unwrap();

        assert!(!settings.api_key.is_empty());
    }

    #[test]
    fn rejects_invalid_settings() {
        for json in [
            "{}",
            r#"{"api-key":42}"#,
            r#"{"api-key":"key","extra":true}"#,
        ] {
            assert!(Settings::from_json(json).is_err());
        }
    }

    #[test]
    fn exposes_search_and_fetch_tools() {
        let definitions = tool_definitions();

        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            [WEB_SEARCH, WEB_FETCH]
        );
        for definition in definitions {
            serde_json::from_str::<serde_json::Value>(&definition.parameters).unwrap();
        }
    }

    #[test]
    fn validates_search_arguments() {
        let arguments: SearchArguments =
            parse_arguments(WEB_SEARCH, r#"{"query":"rust"}"#).unwrap();
        assert_eq!(arguments.limit, DEFAULT_SEARCH_LIMIT);
        arguments.validate().unwrap();

        let arguments: SearchArguments =
            parse_arguments(WEB_SEARCH, r#"{"query":" ","limit":21}"#).unwrap();
        assert!(arguments.validate().is_err());
    }

    #[test]
    fn encodes_v1_request_bodies() {
        let search = serde_json::to_value(SearchRequest {
            query: "rust",
            workflow: "search",
            format: "json",
            limit: 5,
        })
        .unwrap();
        assert_eq!(
            search,
            serde_json::json!({
                "query": "rust",
                "workflow": "search",
                "format": "json",
                "limit": 5
            })
        );

        let extract = serde_json::to_value(ExtractRequest {
            pages: vec![PageInput {
                url: "https://example.com",
            }],
            format: "json",
        })
        .unwrap();
        assert_eq!(
            extract,
            serde_json::json!({
                "pages": [{"url": "https://example.com"}],
                "format": "json"
            })
        );
    }

    #[test]
    fn validates_fetch_urls() {
        let arguments: FetchArguments =
            parse_arguments(WEB_FETCH, r#"{"urls":["https://example.com/article"]}"#).unwrap();
        assert_eq!(arguments.max_chars_per_page, DEFAULT_MAX_CHARS);
        arguments.validate().unwrap();

        let arguments: FetchArguments =
            parse_arguments(WEB_FETCH, r#"{"urls":["http://example.com"]}"#).unwrap();
        assert_eq!(
            arguments.validate().unwrap_err(),
            "web fetch URL must be HTTPS: `http://example.com`"
        );
    }

    #[test]
    fn converts_search_results_to_a_small_stable_shape() {
        let body = r#"{
            "data":{"search":[
                {"url":"https://example.com/one","title":"One &amp; Only","snippet":"It&#39;s first","time":"2026-08-10T00:00:00Z"},
                {"url":"https://example.com/two","title":"Two","snippet":"Second"}
            ]}
        }"#;

        let output: serde_json::Value =
            serde_json::from_str(&search_output(body, 1).unwrap()).unwrap();

        assert_eq!(output["results"].as_array().unwrap().len(), 1);
        assert_eq!(output["results"][0]["title"], "One & Only");
        assert_eq!(output["results"][0]["snippet"], "It's first");
        assert_eq!(output["results"][0]["published"], "2026-08-10T00:00:00Z");
    }

    #[test]
    fn truncates_extracted_markdown_on_character_boundaries() {
        let body = r#"{
            "data":[
                {"url":"https://example.com/one","markdown":"ab🦀cd"},
                {"url":"https://example.com/two","error":"timed out"}
            ]
        }"#;

        let output: serde_json::Value =
            serde_json::from_str(&extract_output(body, 3).unwrap()).unwrap();

        assert_eq!(output["pages"][0]["markdown"], "ab🦀");
        assert_eq!(output["pages"][0]["truncated"], true);
        assert_eq!(output["pages"][1]["error"], "timed out");
        assert!(output["pages"][1].get("truncated").is_none());
    }

    #[test]
    fn reports_structured_api_errors() {
        let error = api_error(
            401,
            r#"{"error":[{"code":"auth.invalid","message":"Invalid API key"}]}"#,
        );

        assert_eq!(error, "Kagi API returned HTTP 401: Invalid API key");
    }
}

bindings::export!(Kagi with_types_in bindings);
