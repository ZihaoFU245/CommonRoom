use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Tavily search endpoint. Override with `CHAT_SEARCH_URL` to point at a proxy.
const DEFAULT_ENDPOINT: &str = "https://api.tavily.com/search";
const SEARCH_URL_ENV: &str = "CHAT_SEARCH_URL";
/// Enough sources to answer with, few enough to keep the prompt small.
const MAX_RESULTS: u8 = 5;
/// Characters of each page kept for the answer prompt.
const SNIPPET: usize = 400;
const TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    results: Vec<Hit>,
}
#[derive(Deserialize, Serialize, Clone)]
pub struct Hit {
    pub title: String,
    pub url: String,
    #[serde(default)]
    content: String,
}

/// One search result as handed to the model and quoted in the answer.
#[derive(Clone)]
pub struct Source {
    pub title: String,
    pub url: String,
}

pub struct Findings {
    pub text: String,
    pub sources: Vec<Source>,
}

fn endpoint() -> String {
    std::env::var(SEARCH_URL_ENV)
        .ok()
        .filter(|value| value.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_ENDPOINT.into())
}

/// Search the web for one query. `key` is the agent's own search credential.
pub(super) async fn search(
    http: &reqwest::Client,
    key: &str,
    query: &str,
) -> Result<Findings, String> {
    // The query is logged so a poor result set can be traced to it. It is
    // message text, never a credential.
    tracing::info!(query = %query, "Searching the web");
    let body = serde_json::json!({
        "query": query,
        "max_results": MAX_RESULTS,
        "search_depth": "basic",
        "include_answer": false,
        "include_raw_content": false,
    });
    let response = http
        .post(endpoint())
        .header("authorization", format!("Bearer {key}"))
        .json(&body)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            // The credential is never part of the logged error.
            tracing::warn!(error = %error, "Search request failed");
            "The search provider could not be reached.".to_string()
        })?;
    let status = response.status();
    let raw = response.text().await.unwrap_or_default();
    if !status.is_success() {
        tracing::warn!(status = status.as_u16(), "Search request rejected");
        return Err(format!(
            "The search provider rejected the request ({}). Check the search key with /agent-search-key.",
            status.as_u16()
        ));
    }
    let response: Response = serde_json::from_str(&raw).map_err(|error| {
        tracing::warn!(error = %error, "Search results could not be read");
        "The search results could not be read.".to_string()
    })?;
    findings(response.results)
}

fn findings(hits: Vec<Hit>) -> Result<Findings, String> {
    let hits: Vec<Hit> = hits
        .into_iter()
        .filter(|hit| hit.url.starts_with("https://") || hit.url.starts_with("http://"))
        .collect();
    if hits.is_empty() {
        return Err("The search found no usable results.".into());
    }
    let text = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            let snippet: String = hit.content.split_whitespace().collect::<Vec<_>>().join(" ");
            let snippet: String = snippet.chars().take(SNIPPET).collect();
            // The address is deliberately absent: the model cites a result by
            // number, and the source list below the answer carries the link.
            format!("[{}] {}\n{}", index + 1, hit.title.trim(), snippet)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(Findings {
        text,
        sources: hits
            .into_iter()
            .map(|hit| Source {
                title: hit.title.trim().to_string(),
                url: hit.url.trim().to_string(),
            })
            .collect(),
    })
}

/// Characters of a page title quoted in the source list. Long titles make an
/// answer unreadable, and the link itself carries the full address.
const TITLE: usize = 40;

/// The marker an answer ends with to list the results it used. The model
/// decides which sources to show, so a search result it did not need never
/// appears as a reference.
pub(super) const CITATION: &str = "引用：";

/// Split an answer into its text and the result numbers the model cited.
///
/// The scanner walks the citation line and keeps every plain number, so
/// whatever way the model separates them the same numbers are read.
pub(super) fn split_citations(answer: &str) -> (String, Vec<usize>) {
    let Some(start) = answer.rfind(CITATION) else {
        return (answer.trim().to_string(), Vec::new());
    };
    let mut numbers = Vec::new();
    let mut current = String::new();
    for character in answer[start + CITATION.len()..].chars() {
        if character.is_ascii_digit() {
            current.push(character);
        } else {
            if let Ok(value) = current.parse::<usize>() {
                numbers.push(value);
            }
            current.clear();
        }
    }
    if let Ok(value) = current.parse::<usize>() {
        numbers.push(value);
    }
    numbers.retain(|number| *number > 0);
    (answer[..start].trim().to_string(), numbers)
}

/// Keep the sources the answer cites, in the order the answer cites them.
/// An answer that cites nothing returns none, so a question that needed no
/// sources is not followed by a list of them.
pub(super) fn cited_sources(sources: &[Source], cited: &[usize]) -> Vec<Source> {
    let mut chosen: Vec<Source> = Vec::new();
    for number in cited {
        if let Some(source) = sources.get(number - 1)
            && !chosen.iter().any(|seen| seen.url == source.url)
        {
            chosen.push(source.clone());
        }
    }
    chosen
}

/// Append the sources an answer used as a short link list. Only the label is
/// shown, so a long address never clutters the message.
pub(super) fn with_sources(answer: &str, sources: &[Source]) -> String {
    if sources.is_empty() {
        return answer.trim_end().to_string();
    }
    let list = sources
        .iter()
        .map(|source| format!("- [{}]({})", short_title(&source.title), source.url))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n\n来源：\n{}", answer.trim_end(), list)
}

/// A readable label for one source, or a plain word when a page has no title.
fn short_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return "来源".into();
    }
    if title.chars().count() <= TITLE {
        return title;
    }
    let head: String = title.chars().take(TITLE).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests {
    use super::*;
    fn hit(title: &str, url: &str, content: &str) -> Hit {
        Hit {
            title: title.into(),
            url: url.into(),
            content: content.into(),
        }
    }
    #[test]
    fn sources_are_numbered_and_limited_to_readable_text() {
        let found = findings(vec![
            hit("First", "https://example.com/a", "one   two\nthree"),
            hit("Second", "http://example.com/b", "x"),
        ])
        .unwrap();
        assert_eq!(found.sources.len(), 2);
        assert_eq!(found.sources[0].url, "https://example.com/a");
        assert!(
            found.text.starts_with("[1] First\none two three"),
            "the prompt text carries no address: {}",
            found.text
        );
        assert!(found.text.contains("[2] Second"));
    }
    #[test]
    fn unusable_results_are_reported_rather_than_answered() {
        assert!(findings(Vec::new()).is_err());
        // A scheme a chat client cannot open is not a source.
        assert!(findings(vec![hit("Odd", "javascript:alert(1)", "body")]).is_err());
        assert!(findings(vec![hit("Local", "/relative/path", "body")]).is_err());
    }
    #[test]
    fn long_snippets_are_trimmed_and_sources_are_appended() {
        let found = findings(vec![hit(
            "Long",
            "https://example.com/long",
            &"词".repeat(900),
        )])
        .unwrap();
        let body = found.text.lines().last().unwrap_or_default();
        assert_eq!(body.chars().count(), SNIPPET);
        let answer = with_sources("答案", &found.sources);
        assert!(answer.starts_with("答案\n\n来源：\n- [Long](https://example.com/long)"));
        assert_eq!(with_sources("答案", &[]), "答案");
        // An answer with no sources is trimmed, so a citation line the model
        // left behind never leaves a trailing gap.
        assert_eq!(with_sources("答案\n\n", &[]), "答案");
    }
    #[test]
    fn the_prompt_never_contains_an_address_to_copy() {
        let found = findings(vec![hit(
            "Title",
            "https://example.com/blog/%E4%B8%AD%E6%96%87",
            "body",
        )])
        .unwrap();
        assert!(
            !found.text.contains("https://"),
            "a URL the model could copy back is never in the prompt: {}",
            found.text
        );
        assert!(found.text.contains("[1] Title"));
        // The link the reader gets is the real, complete address.
        assert_eq!(
            found.sources[0].url,
            "https://example.com/blog/%E4%B8%AD%E6%96%87"
        );
    }
    #[test]
    fn source_labels_are_short_and_never_empty() {
        assert_eq!(short_title("  DeepSeek   Docs  "), "DeepSeek Docs");
        assert_eq!(short_title(""), "来源");
        assert_eq!(short_title("   "), "来源");
        let long = short_title(&"标".repeat(80));
        assert_eq!(long.chars().count(), TITLE + 1);
        assert!(long.ends_with('…'));
        let found = findings(vec![hit("", "https://example.com/a", "b")]).unwrap();
        assert!(with_sources("答案", &found.sources).contains("- [来源](https://example.com/a)"));
    }
    #[test]
    fn only_cited_results_become_sources() {
        let sources = vec![
            Source {
                title: "First".into(),
                url: "https://example.com/a".into(),
            },
            Source {
                title: "Second".into(),
                url: "https://example.com/b".into(),
            },
            Source {
                title: "Third".into(),
                url: "https://example.com/c".into(),
            },
        ];
        // A model that writes one number per line, or all on one line, reads
        // the same.
        for cited in ["引用：\n1\n2", "引用：1, 2", "引用：1、2", "引用：1 2"] {
            let (answer, numbers) = split_citations(&format!("答案\n\n{cited}"));
            assert_eq!(answer, "答案", "{cited}");
            assert_eq!(numbers, vec![1, 2], "{cited}");
        }
        // Order and duplicates follow the answer, not the result set.
        let (_, numbers) = split_citations("答案\n引用：2, 1, 2");
        assert_eq!(numbers, vec![2, 1, 2]);
        assert_eq!(
            cited_sources(&sources, &numbers)
                .iter()
                .map(|source| source.url.as_str())
                .collect::<Vec<_>>(),
            ["https://example.com/b", "https://example.com/a"]
        );
        // An invented number is ignored rather than trusted.
        let (_, numbers) = split_citations("答案\n引用：9 1");
        assert_eq!(
            cited_sources(&sources, &numbers).len(),
            1,
            "an out-of-range citation selects nothing"
        );
        // No citation line, or none of the results used, means no list at all.
        assert_eq!(split_citations("答案只有正文").1.len(), 0);
        assert_eq!(
            with_sources("答案", &cited_sources(&sources, &[])),
            "答案",
            "an uncited answer stays short"
        );
        // A citation line with no answer above it is still just an answer.
        let (answer, numbers) = split_citations("引用：1");
        assert_eq!(answer, "");
        assert_eq!(numbers, vec![1]);
    }
}
