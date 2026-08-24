//! The verification engine (spec §8) — prompt, tool loop, Exa search, both providers.
//!
//! Design load-bearing points, each tied to a real shipped bug:
//!   * `unverifiable` means the evidence does not SETTLE the claim — not "I didn't
//!     look", and NOT the safe default. Without saying this plainly, the model treats
//!     grey as caution and nearly everything comes back grey.
//!   * The model may answer stable facts from its own knowledge (with disclosure), but
//!     `must_look_it_up` forces a search where recall is the wrong instrument (rates,
//!     recency, attribution, scripture).
//!   * Prefetch: for a claim we already know needs a search, run it FIRST with the
//!     claim as the query and seed the results into the opening turn, so the model
//!     reaches a verdict on its first call instead of spending a round trip naming a
//!     query. Seed those sources into provenance so the verdict doesn't appear from
//!     nowhere.

use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

// --- Verdict / provenance types (mirror the frontend) ---

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Verified,
    False,
    Misleading,
    ContextNeeded,
    Unverifiable,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    OpenAI,
    Anthropic,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Source {
    pub title: String,
    pub url: String,
    #[serde(rename = "publishedDate", skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Provenance {
    pub web_search_used: bool,
    pub provider: Provider,
    pub model: String,
    pub latency_ms: u64,
    pub sources: Vec<Source>,
    pub prefetched: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct VerdictResult {
    pub claim: String,
    pub verdict: Verdict,
    pub rationale: String,
    pub correction: Option<String>,
    pub provenance: Provenance,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("no verification provider configured")]
    NoProvider,
    #[error("verification saturated — skipped")]
    Saturated,
    #[error("verification failed: {0}")]
    Failed(String),
}

// --- must_look_it_up + figure/scripture detection (pure, unit-tested) ---

static PERCENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(\d+(\.\d+)?\s*%|\d+(\.\d+)?\s*percent|per\s*cent|per\s+capita|\brate\b)").unwrap());
static DIGIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\d").unwrap());
static SPELLED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million|billion|trillion)\b").unwrap()
});
static RECENCY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(currently|current|recent|recently|this year|last year|so far this year|latest|record (high|low)|all[- ]time|up from|down from|as of|now|today|this month|this quarter)\b").unwrap()
});
static ATTRIBUTION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(according to|said|claimed|stated|announced|reported|told reporters|testified|tweeted|wrote)\b").unwrap()
});
static SCRIPTURE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(qur'?an|quran|hadith|bible|biblical|gospel|torah|talmud|tanakh|bhagavad|gita|vedas?|upanishad|guru granth|tripitaka|sutra|scripture|scriptural|verse|surah|psalm|proverb|the prophet|thou shalt|book of \w+)\b").unwrap()
});

/// Any figure at all — digits or spelled-out numerals. Drives the statistics prompt.
pub fn has_figure(text: &str) -> bool {
    DIGIT_RE.is_match(text) || SPELLED_RE.is_match(text)
}

pub fn cites_scripture(text: &str) -> bool {
    SCRIPTURE_RE.is_match(text)
}

/// Force a search where the model's memory is the wrong instrument. Returning false
/// lets a stable fact be answered from knowledge; returning true means recall would be
/// unreliable (a measurement that moves, something current, an attribution, scripture).
pub fn must_look_it_up(text: &str) -> bool {
    // A rate/percentage is a measurement, and measurements move.
    if PERCENT_RE.is_match(text) {
        return true;
    }
    // Anything current/recent — the model's training cutoff can't settle it.
    if RECENCY_RE.is_match(text) {
        return true;
    }
    // Anything attributed to a named person or report — check the source, don't recall.
    if ATTRIBUTION_RE.is_match(text) {
        return true;
    }
    // Any quoted religious/scriptural text — wording and citation must be checked.
    if cites_scripture(text) {
        return true;
    }
    false
}

// --- Prompt assembly. Conditional sections are appended ONLY when relevant, so a
//     plain claim doesn't pay for scripture/statistics guidance it doesn't need. ---

const BASE_PROMPT: &str = r#"You are Verity, a real-time fact checker. You are given a single spoken claim and any relevant debate context. Reach ONE verdict and report it by calling submit_verdict. Do not answer in prose.

Verdicts:
- verified: the claim is accurate.
- false: contradicted by the evidence.
- misleading: technically defensible but arranged to create a false impression. This is the MOST IMPORTANT verdict and the easiest to miss: a true statement can still deceive, and that survives a spot check. When a real number carries a false impression, call it misleading and state the real number.
- context_needed: the claim depends on a definition, timeframe, or baseline that was not stated, and the answer changes depending on it.
- unverifiable: the evidence does not SETTLE the claim either way, or it is opinion / prediction / a value judgement.

Critical rules about unverifiable:
- unverifiable means the evidence does not settle it. It does NOT mean "I did not look", and it is NOT the safe default. A grey verdict on a settleable claim is a FAILURE, not caution.
- You MAY answer from your own knowledge for stable facts (settled history, geography, science, definitions), provided you say you did so and keep your confidence honest. Do not spend a search confirming the war ended in 1945.
- Keep confidence honest: if you are reasoning from memory on something that may have changed, say so."#;

const SEARCH_AVAILABLE: &str = r#"

You have a web-search tool (search_web). Use it when recall is the wrong instrument — any rate or percentage, anything current or recent, anything attributed to a named person or report, or any quoted scripture. Prefer the evidence you were given before searching again."#;

const SEARCH_UNAVAILABLE: &str = r#"

No web-search tool is available. Judge from your own knowledge where you honestly can, and disclose that you did. Do NOT retreat to unverifiable merely because you could not search — reserve unverifiable for claims the evidence genuinely cannot settle."#;

const STATISTICS_SECTION: &str = r#"

STATISTICS: A figure is present. Check the FIGURE, not the sentiment around it. A figure is only true relative to a population, a place, and a period; if one of those is unstated and the answer changes depending on it, that is context_needed. State what the real number is. A real number carrying a false impression is misleading, not verified."#;

const SCRIPTURE_SECTION: &str = r#"

SCRIPTURE: A religious text or tradition-teaching is cited. Quoting scripture out of context is one of the commonest ways a true-sounding statement misleads, and unlike most rhetoric it is checkable. Check the wording and that the citation exists; report the surrounding verses and any condition or addressee. A verse quoted accurately but stripped of qualifying context is misleading, not verified. Separate what the text says, how scholars of that tradition read it, and what an individual believes.
Two limits, non-negotiable: do NOT rule on theology or whose interpretation is correct — that is not a factual question, so such a claim is context_needed. Hold every tradition (Quran, Hadith, Bible, Torah, Talmud, Gita, Vedas, Guru Granth, Tripitaka and others) to the SAME standard whichever way the framing runs. A checker that scrutinises one faith more closely than another is itself a source of distortion."#;

/// Build the system prompt for a claim. Appends the statistics section when a figure is
/// present and the scripture section when a religious text is cited.
pub fn build_system_prompt(claim: &str, web_search_available: bool) -> String {
    let mut p = String::from(BASE_PROMPT);
    p.push_str(if web_search_available { SEARCH_AVAILABLE } else { SEARCH_UNAVAILABLE });
    if has_figure(claim) {
        p.push_str(STATISTICS_SECTION);
    }
    if cites_scripture(claim) {
        p.push_str(SCRIPTURE_SECTION);
    }
    p
}

/// The user turn: the claim, the debate brief, and any prefetched evidence.
pub fn build_user_message(claim: &str, brief: &str, prefetched: &[Source]) -> String {
    let mut m = String::new();
    if !brief.trim().is_empty() {
        m.push_str("DEBATE CONTEXT (earlier turns and settled verdicts):\n");
        m.push_str(brief.trim());
        m.push_str("\n\n");
    }
    if !prefetched.is_empty() {
        m.push_str("EVIDENCE (already retrieved for this claim):\n");
        for (i, s) in prefetched.iter().enumerate() {
            let date = s.published_date.as_deref().unwrap_or("n.d.");
            m.push_str(&format!(
                "[{}] {} ({}) {}\n{}\n\n",
                i + 1,
                s.title,
                date,
                s.url,
                truncate(&s.text, 1200)
            ));
        }
    }
    m.push_str("CLAIM TO CHECK:\n");
    m.push_str(claim.trim());
    m
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

// --- Tool schemas (strict). The verdict comes back via a TOOL, not a response format,
//     so the whole exchange stays in one mechanism. ---

fn verdict_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "verdict": {
                "type": "string",
                "enum": ["verified", "false", "misleading", "context_needed", "unverifiable"]
            },
            "rationale": { "type": "string", "description": "One or two sentences. State the real number if relevant." },
            "correction": { "type": "string", "description": "The corrected fact/number, or empty if none." }
        },
        "required": ["verdict", "rationale", "correction"],
        "additionalProperties": false
    })
}

fn search_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "A focused web search query." }
        },
        "required": ["query"],
        "additionalProperties": false
    })
}

/// OpenAI-format tools. search_web is WITHHELD when web search is unavailable — a model
/// with no search tool cannot pretend to have searched.
pub fn openai_tools(web_search_available: bool) -> Vec<serde_json::Value> {
    let mut tools = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "submit_verdict",
            "description": "Submit the final verdict. Calling this ends the check.",
            "strict": true,
            "parameters": verdict_schema()
        }
    })];
    if web_search_available {
        tools.insert(0, serde_json::json!({
            "type": "function",
            "function": {
                "name": "search_web",
                "description": "Search the web for evidence. Returns titles, URLs, dates and snippets.",
                "strict": true,
                "parameters": search_schema()
            }
        }));
    }
    tools
}

/// Anthropic-format tools.
pub fn anthropic_tools(web_search_available: bool) -> Vec<serde_json::Value> {
    let mut tools = vec![serde_json::json!({
        "name": "submit_verdict",
        "description": "Submit the final verdict. Calling this ends the check.",
        "input_schema": verdict_schema()
    })];
    if web_search_available {
        tools.insert(0, serde_json::json!({
            "name": "search_web",
            "description": "Search the web for evidence. Returns titles, URLs, dates and snippets.",
            "input_schema": search_schema()
        }));
    }
    tools
}

// --- Exa search ---

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExaMode {
    /// Exa's real-time tier, documented for chat / voice agents — used for the prefetch.
    Instant,
    /// A follow-up the model earned after seeing the prefetch was insufficient.
    Fast,
}

impl ExaMode {
    fn as_str(&self) -> &'static str {
        match self {
            ExaMode::Instant => "instant",
            ExaMode::Fast => "fast",
        }
    }
}

/// Build the Exa request body. The `contents.text` block is CRITICAL: without it Exa
/// returns bare titles and URLs with no page text, the model gets citations with no
/// evidence behind them, and you land back on unverifiable with a search bill attached.
pub fn exa_request_body(query: &str, mode: ExaMode) -> serde_json::Value {
    serde_json::json!({
        "query": query,
        "numResults": 5,
        "type": "auto",
        "mode": mode.as_str(),
        "contents": {
            "text": { "maxCharacters": 1200 }
        }
    })
}

/// Parse Exa's response into sources, capturing publishedDate (recency decides a whole
/// class of claims). Exa returns no relevance score; that's fine.
pub fn parse_exa_response(body: &serde_json::Value) -> Vec<Source> {
    body.get("results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| Source {
                    title: r.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    url: r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    published_date: r
                        .get("publishedDate")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    text: r.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn exa_search(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
    mode: ExaMode,
) -> Result<Vec<Source>, String> {
    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .json(&exa_request_body(query, mode))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parse_exa_response(&body))
}

// --- Provider selection ---

pub fn choose_provider(
    openai_key: Option<&str>,
    anthropic_key: Option<&str>,
    preference: &str,
) -> Option<Provider> {
    let has_openai = openai_key.map(|k| !k.is_empty()).unwrap_or(false);
    let has_anthropic = anthropic_key.map(|k| !k.is_empty()).unwrap_or(false);
    match preference {
        "openai" => has_openai.then_some(Provider::OpenAI),
        "anthropic" => has_anthropic.then_some(Provider::Anthropic),
        _ => {
            if has_openai {
                Some(Provider::OpenAI)
            } else if has_anthropic {
                Some(Provider::Anthropic)
            } else {
                None
            }
        }
    }
}

// --- The engine ---

pub const MAX_ROUNDS: usize = 4;
pub const MAX_CONCURRENCY: usize = 3;

#[derive(Clone)]
pub struct VerifyEngine {
    pub client: reqwest::Client,
    pub openai_key: Option<String>,
    pub anthropic_key: Option<String>,
    pub exa_key: Option<String>,
    pub web_search_enabled: bool,
    pub provider_pref: String,
    pub openai_model: String,
    pub anthropic_model: String,
    sem: Arc<tokio::sync::Semaphore>,
}

impl VerifyEngine {
    pub fn new(
        openai_key: Option<String>,
        anthropic_key: Option<String>,
        exa_key: Option<String>,
        web_search_enabled: bool,
        provider_pref: String,
        openai_model: String,
        anthropic_model: String,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            openai_key,
            anthropic_key,
            exa_key,
            web_search_enabled,
            provider_pref,
            openai_model,
            anthropic_model,
            sem: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENCY)),
        }
    }

    /// Construct with a shared HTTP client and semaphore so the concurrency cap is
    /// GLOBAL across every verify_claim call, not per-call. The caller (AppState) owns
    /// the semaphore for the app's lifetime.
    #[allow(clippy::too_many_arguments)]
    pub fn with_shared(
        client: reqwest::Client,
        sem: Arc<tokio::sync::Semaphore>,
        openai_key: Option<String>,
        anthropic_key: Option<String>,
        exa_key: Option<String>,
        web_search_enabled: bool,
        provider_pref: String,
        openai_model: String,
        anthropic_model: String,
    ) -> Self {
        Self {
            client,
            openai_key,
            anthropic_key,
            exa_key,
            web_search_enabled,
            provider_pref,
            openai_model,
            anthropic_model,
            sem,
        }
    }

    fn web_search_available(&self) -> bool {
        self.web_search_enabled && self.exa_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
    }

    /// Verify one claim. Skips (rather than queues) when all concurrency slots are busy
    /// — a late verdict is worthless, so a slow moment should drop a check, not delay
    /// every later one behind it.
    pub async fn verify(&self, claim: &str, brief: &str) -> Result<VerdictResult, VerifyError> {
        let _permit = self.sem.try_acquire().map_err(|_| VerifyError::Saturated)?;
        let provider = choose_provider(
            self.openai_key.as_deref(),
            self.anthropic_key.as_deref(),
            &self.provider_pref,
        )
        .ok_or(VerifyError::NoProvider)?;

        let started = std::time::Instant::now();
        let web_available = self.web_search_available();

        // Prefetch: for a claim we already know needs a search, the query is the claim.
        // Run it first (instant tier) and seed the results into the opening turn.
        let mut prefetched: Vec<Source> = Vec::new();
        let mut used_search = false;
        if web_available && must_look_it_up(claim) {
            if let Some(exa) = self.exa_key.as_deref() {
                match exa_search(&self.client, exa, claim, ExaMode::Instant).await {
                    Ok(mut s) => {
                        used_search = !s.is_empty();
                        prefetched.append(&mut s);
                    }
                    // A failed prefetch must not be fatal — losing the head start beats
                    // losing the verdict.
                    Err(e) => log::warn!("prefetch failed (continuing): {e}"),
                }
            }
        }

        let result = match provider {
            Provider::OpenAI => self.run_openai(claim, brief, &prefetched, web_available).await,
            Provider::Anthropic => self.run_anthropic(claim, brief, &prefetched, web_available).await,
        };

        let (verdict, rationale, correction, mut sources, followup_searched) =
            result.map_err(VerifyError::Failed)?;

        // Seed prefetched sources into provenance even though the model never called the
        // tool for them — a verdict resting on them must not appear from nowhere.
        let mut all_sources = prefetched.clone();
        all_sources.append(&mut sources);
        dedup_sources(&mut all_sources);

        Ok(VerdictResult {
            claim: claim.to_string(),
            verdict,
            rationale,
            correction: correction.filter(|c| !c.trim().is_empty()),
            provenance: Provenance {
                web_search_used: used_search || followup_searched,
                provider,
                model: match provider {
                    Provider::OpenAI => self.openai_model.clone(),
                    Provider::Anthropic => self.anthropic_model.clone(),
                },
                latency_ms: started.elapsed().as_millis() as u64,
                sources: all_sources,
                prefetched: !prefetched.is_empty(),
            },
        })
    }

    async fn run_openai(
        &self,
        claim: &str,
        brief: &str,
        prefetched: &[Source],
        web_available: bool,
    ) -> Result<(Verdict, String, Option<String>, Vec<Source>, bool), String> {
        let key = self.openai_key.clone().ok_or("no openai key")?;
        let tools = openai_tools(web_available);
        let mut messages = vec![
            serde_json::json!({ "role": "system", "content": build_system_prompt(claim, web_available) }),
            serde_json::json!({ "role": "user", "content": build_user_message(claim, brief, prefetched) }),
        ];
        let mut followup_searched = false;
        let mut collected: Vec<Source> = Vec::new();

        for round in 0..MAX_ROUNDS {
            let last = round == MAX_ROUNDS - 1;
            // Force submit_verdict on the last round so a model that keeps searching
            // still produces an answer.
            let tool_choice = if last {
                serde_json::json!({ "type": "function", "function": { "name": "submit_verdict" } })
            } else {
                serde_json::json!("auto")
            };
            let body = serde_json::json!({
                "model": self.openai_model,
                "messages": messages,
                "tools": tools,
                "tool_choice": tool_choice,
                "temperature": 0.1
            });
            let resp: serde_json::Value = self
                .client
                .post("https://api.openai.com/v1/chat/completions")
                .bearer_auth(&key)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;

            let choice = resp
                .get("choices")
                .and_then(|c| c.get(0))
                .ok_or_else(|| format!("openai: no choices: {resp}"))?;
            let message = choice.get("message").ok_or("openai: no message")?;
            let tool_calls = message.get("tool_calls").and_then(|t| t.as_array()).cloned();

            let Some(calls) = tool_calls else {
                // No tool call and not forced — nudge once more, else fail out.
                messages.push(message.clone());
                continue;
            };

            // Echo the assistant turn so tool results attach correctly.
            messages.push(message.clone());

            for call in &calls {
                let name = call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let args_str = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("{}");
                let args: serde_json::Value = serde_json::from_str(args_str).unwrap_or_default();
                let call_id = call.get("id").and_then(|i| i.as_str()).unwrap_or("");

                if name == "submit_verdict" {
                    return Ok(parse_verdict_args(&args, collected));
                } else if name == "search_web" {
                    followup_searched = true;
                    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or(claim);
                    let mut results = if let Some(exa) = self.exa_key.as_deref() {
                        exa_search(&self.client, exa, query, ExaMode::Fast)
                            .await
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let rendered = render_sources(&results);
                    collected.append(&mut results);
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": rendered
                    }));
                }
            }
        }
        // Forced submit on the last round should prevent reaching here.
        Ok((Verdict::Unverifiable, "No verdict was produced.".into(), None, collected, followup_searched))
    }

    async fn run_anthropic(
        &self,
        claim: &str,
        brief: &str,
        prefetched: &[Source],
        web_available: bool,
    ) -> Result<(Verdict, String, Option<String>, Vec<Source>, bool), String> {
        let key = self.anthropic_key.clone().ok_or("no anthropic key")?;
        let tools = anthropic_tools(web_available);
        let mut messages = vec![serde_json::json!({
            "role": "user",
            "content": build_user_message(claim, brief, prefetched)
        })];
        let system = build_system_prompt(claim, web_available);
        let mut followup_searched = false;
        let mut collected: Vec<Source> = Vec::new();

        for round in 0..MAX_ROUNDS {
            let last = round == MAX_ROUNDS - 1;
            let tool_choice = if last {
                serde_json::json!({ "type": "tool", "name": "submit_verdict" })
            } else {
                serde_json::json!({ "type": "auto" })
            };
            let body = serde_json::json!({
                "model": self.anthropic_model,
                "max_tokens": 1024,
                "system": system,
                "messages": messages,
                "tools": tools,
                "tool_choice": tool_choice
            });
            let resp: serde_json::Value = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;

            let content = resp
                .get("content")
                .and_then(|c| c.as_array())
                .cloned()
                .ok_or_else(|| format!("anthropic: no content: {resp}"))?;

            // Echo assistant turn.
            messages.push(serde_json::json!({ "role": "assistant", "content": content.clone() }));

            let mut tool_results = Vec::new();
            for block in &content {
                if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                    continue;
                }
                let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let input = block.get("input").cloned().unwrap_or_default();
                let use_id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");

                if name == "submit_verdict" {
                    return Ok(parse_verdict_args(&input, collected));
                } else if name == "search_web" {
                    followup_searched = true;
                    let query = input.get("query").and_then(|q| q.as_str()).unwrap_or(claim);
                    let mut results = if let Some(exa) = self.exa_key.as_deref() {
                        exa_search(&self.client, exa, query, ExaMode::Fast)
                            .await
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let rendered = render_sources(&results);
                    collected.append(&mut results);
                    tool_results.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": use_id,
                        "content": rendered
                    }));
                }
            }
            if !tool_results.is_empty() {
                messages.push(serde_json::json!({ "role": "user", "content": tool_results }));
            }
        }
        Ok((Verdict::Unverifiable, "No verdict was produced.".into(), None, collected, followup_searched))
    }
}

fn parse_verdict_args(
    args: &serde_json::Value,
    collected: Vec<Source>,
) -> (Verdict, String, Option<String>, Vec<Source>, bool) {
    let verdict = match args.get("verdict").and_then(|v| v.as_str()).unwrap_or("unverifiable") {
        "verified" => Verdict::Verified,
        "false" => Verdict::False,
        "misleading" => Verdict::Misleading,
        "context_needed" => Verdict::ContextNeeded,
        _ => Verdict::Unverifiable,
    };
    let rationale = args.get("rationale").and_then(|r| r.as_str()).unwrap_or("").to_string();
    let correction = args
        .get("correction")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    (verdict, rationale, correction, collected, false)
}

fn render_sources(sources: &[Source]) -> String {
    if sources.is_empty() {
        return "No results.".into();
    }
    sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let date = s.published_date.as_deref().unwrap_or("n.d.");
            format!("[{}] {} ({})\n{}\n{}", i + 1, s.title, date, s.url, truncate(&s.text, 1200))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn dedup_sources(sources: &mut Vec<Source>) {
    let mut seen = std::collections::HashSet::new();
    sources.retain(|s| seen.insert(s.url.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn must_look_it_up_forces_where_recall_fails() {
        // Rates / percentages (measurements move).
        assert!(must_look_it_up("unemployment is 4.2%"));
        assert!(must_look_it_up("the murder rate fell"));
        // Current / recent.
        assert!(must_look_it_up("inflation is currently high"));
        assert!(must_look_it_up("it's a record high this year"));
        // Attribution to a named person or report.
        assert!(must_look_it_up("according to the ONS, it rose"));
        assert!(must_look_it_up("the minister claimed it doubled"));
        // Scripture.
        assert!(must_look_it_up("the Quran says to fight the unbelievers"));
    }

    #[test]
    fn must_look_it_up_lets_stable_facts_through() {
        assert!(!must_look_it_up("the second world war ended in 1945"));
        assert!(!must_look_it_up("Paris is the capital of France"));
        assert!(!must_look_it_up("water is made of hydrogen and oxygen"));
    }

    #[test]
    fn prompt_includes_statistics_section_only_with_a_figure() {
        let with = build_system_prompt("unemployment is 8 percent", true);
        assert!(with.contains("STATISTICS"));
        assert!(with.contains("population, a place, and a period"));
        let without = build_system_prompt("Paris is the capital of France", true);
        assert!(!without.contains("STATISTICS"));
    }

    #[test]
    fn prompt_includes_scripture_section_only_when_cited() {
        let with = build_system_prompt("the Bible says turn the other cheek", true);
        assert!(with.contains("SCRIPTURE"));
        assert!(with.contains("SAME standard")); // holds every tradition equally
        let without = build_system_prompt("crime rose last year", true);
        assert!(!without.contains("SCRIPTURE"));
    }

    #[test]
    fn prompt_never_makes_unverifiable_the_safe_default() {
        let p = build_system_prompt("some claim", true);
        assert!(p.contains("NOT the safe default"));
        assert!(p.contains("does not settle"));
    }

    #[test]
    fn search_tool_withheld_when_unavailable() {
        let with = openai_tools(true);
        let names: Vec<&str> = with
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"search_web"));
        assert!(names.contains(&"submit_verdict"));

        let without = openai_tools(false);
        let names: Vec<&str> = without
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(!names.contains(&"search_web"));
        assert!(names.contains(&"submit_verdict"));

        // And the prompt tells the model there is no search rather than implying one.
        assert!(build_system_prompt("x", false).contains("No web-search tool"));
    }

    #[test]
    fn verdict_tool_schema_is_strict() {
        let t = &openai_tools(false)[0];
        assert_eq!(t["function"]["strict"], true);
        assert_eq!(t["function"]["parameters"]["additionalProperties"], false);
        let en = &t["function"]["parameters"]["properties"]["verdict"]["enum"];
        let variants: Vec<&str> = en.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(variants.contains(&"misleading"));
        assert!(variants.contains(&"context_needed"));
    }

    #[test]
    fn exa_body_requests_page_text() {
        let b = exa_request_body("unemployment 4.2%", ExaMode::Instant);
        assert_eq!(b["numResults"], 5);
        assert_eq!(b["mode"], "instant");
        // The critical block: without it Exa returns no evidence.
        assert_eq!(b["contents"]["text"]["maxCharacters"], 1200);
    }

    #[test]
    fn prefetched_evidence_reaches_the_first_user_turn() {
        let sources = vec![Source {
            title: "ONS labour market".into(),
            url: "https://ons.gov.uk/x".into(),
            published_date: Some("2024-05-01".into()),
            text: "The unemployment rate was 4.2%".into(),
        }];
        let msg = build_user_message("unemployment is 4.2%", "", &sources);
        assert!(msg.contains("EVIDENCE"));
        assert!(msg.contains("ONS labour market"));
        assert!(msg.contains("2024-05-01"));
        assert!(msg.contains("CLAIM TO CHECK"));
    }

    #[test]
    fn parse_exa_captures_dates_and_text() {
        let body = serde_json::json!({
            "results": [
                { "title": "A", "url": "https://a.com", "publishedDate": "2023-01-02", "text": "hello" },
                { "title": "B", "url": "https://b.com" }
            ]
        });
        let s = parse_exa_response(&body);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].published_date.as_deref(), Some("2023-01-02"));
        assert_eq!(s[0].text, "hello");
        assert_eq!(s[1].published_date, None);
    }

    #[test]
    fn provider_selection_by_key_and_preference() {
        assert_eq!(choose_provider(Some("k"), None, "auto"), Some(Provider::OpenAI));
        assert_eq!(choose_provider(None, Some("k"), "auto"), Some(Provider::Anthropic));
        assert_eq!(choose_provider(Some("k"), Some("k"), "auto"), Some(Provider::OpenAI));
        assert_eq!(choose_provider(Some("k"), Some("k"), "anthropic"), Some(Provider::Anthropic));
        assert_eq!(choose_provider(None, Some("k"), "openai"), None);
        assert_eq!(choose_provider(None, None, "auto"), None);
        assert_eq!(choose_provider(Some(""), None, "auto"), None); // empty key doesn't count
    }

    #[test]
    fn parse_verdict_maps_all_variants() {
        let mk = |v: &str| serde_json::json!({"verdict": v, "rationale": "r", "correction": ""});
        assert_eq!(parse_verdict_args(&mk("misleading"), vec![]).0, Verdict::Misleading);
        assert_eq!(parse_verdict_args(&mk("false"), vec![]).0, Verdict::False);
        assert_eq!(parse_verdict_args(&mk("verified"), vec![]).0, Verdict::Verified);
        assert_eq!(parse_verdict_args(&mk("context_needed"), vec![]).0, Verdict::ContextNeeded);
        assert_eq!(parse_verdict_args(&mk("garbage"), vec![]).0, Verdict::Unverifiable);
    }

    #[test]
    fn prefetch_uses_instant_and_followup_uses_fast() {
        assert_eq!(exa_request_body("q", ExaMode::Instant)["mode"], "instant");
        assert_eq!(exa_request_body("q", ExaMode::Fast)["mode"], "fast");
    }

    #[test]
    fn anthropic_tools_use_input_schema_and_withhold_search() {
        let with = anthropic_tools(true);
        let names: Vec<&str> = with.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_web"));
        assert!(names.contains(&"submit_verdict"));
        // Anthropic uses input_schema, not the OpenAI parameters wrapper.
        assert!(with.iter().all(|t| t.get("input_schema").is_some()));

        let without = anthropic_tools(false);
        let names: Vec<&str> = without.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(!names.contains(&"search_web"));
    }

    #[test]
    fn user_message_orders_context_evidence_then_claim() {
        let msg = build_user_message("the claim", "prior debate turn", &[]);
        let ctx = msg.find("DEBATE CONTEXT").unwrap();
        let claim = msg.find("CLAIM TO CHECK").unwrap();
        assert!(ctx < claim);
    }

    #[test]
    fn user_message_without_brief_or_evidence_is_just_the_claim() {
        let msg = build_user_message("the claim", "", &[]);
        assert!(!msg.contains("DEBATE CONTEXT"));
        assert!(!msg.contains("EVIDENCE"));
        assert!(msg.contains("CLAIM TO CHECK"));
    }

    #[test]
    fn dedup_sources_removes_repeated_urls() {
        let mut s = vec![
            Source { url: "https://a.com".into(), ..Default::default() },
            Source { url: "https://a.com".into(), ..Default::default() },
            Source { url: "https://b.com".into(), ..Default::default() },
        ];
        dedup_sources(&mut s);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn render_sources_includes_dates_and_handles_empty() {
        assert_eq!(render_sources(&[]), "No results.");
        let rendered = render_sources(&[Source {
            title: "T".into(),
            url: "https://x.com".into(),
            published_date: Some("2024-01-01".into()),
            text: "body".into(),
        }]);
        assert!(rendered.contains("2024-01-01"));
        assert!(rendered.contains("https://x.com"));
    }

    #[test]
    fn statistics_and_scripture_can_both_apply() {
        // "the Quran mentions 40 days" carries both a figure and a scripture citation.
        let p = build_system_prompt("the Quran mentions 40 days of something", true);
        assert!(p.contains("STATISTICS"));
        assert!(p.contains("SCRIPTURE"));
    }

    #[test]
    fn scripture_forces_a_lookup_across_traditions() {
        for t in ["the Torah says", "the Hadith records", "the Gita teaches", "the Guru Granth"] {
            assert!(must_look_it_up(t), "{t} should force a lookup");
        }
    }

    #[test]
    fn has_figure_covers_digits_and_words_and_rejects_none() {
        assert!(has_figure("it rose 8 points"));
        assert!(has_figure("about forty of them"));
        assert!(!has_figure("the weather was pleasant"));
    }

    #[test]
    fn cites_scripture_positive_and_negative() {
        assert!(cites_scripture("as the surah explains"));
        assert!(cites_scripture("Psalm 23 says"));
        assert!(!cites_scripture("the quarterly report says"));
    }

    #[test]
    fn choose_provider_prefers_openai_when_both_present_and_auto() {
        assert_eq!(choose_provider(Some("a"), Some("b"), "auto"), Some(Provider::OpenAI));
    }
}
