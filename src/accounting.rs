//! Model pricing, token accounting, and CSV call-cost persistence.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::env;

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use reqwest::Client;
use scraper::Html;
use serde::{Deserialize, Serialize};

use crate::config::AccountingConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Token usage broken out by text, audio, and cached text categories.
pub struct TokenUsage {
    #[serde(default)]
    pub input_text_tokens: u64,
    #[serde(default)]
    pub cached_input_text_tokens: u64,
    #[serde(default)]
    pub output_text_tokens: u64,
    #[serde(default)]
    pub input_audio_tokens: u64,
    #[serde(default)]
    pub output_audio_tokens: u64,
}

impl TokenUsage {
    /// Returns the total token count across all tracked categories.
    pub fn total_tokens(&self) -> u64 {
        self.input_text_tokens
            + self.output_text_tokens
            + self.input_audio_tokens
            + self.output_audio_tokens
    }

    fn merge_from(&mut self, other: &Self) {
        self.input_text_tokens += other.input_text_tokens;
        self.cached_input_text_tokens += other.cached_input_text_tokens;
        self.output_text_tokens += other.output_text_tokens;
        self.input_audio_tokens += other.input_audio_tokens;
        self.output_audio_tokens += other.output_audio_tokens;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A single persisted API-call accounting row.
pub struct ApiCallLogEntry {
    pub at: String,
    pub call_id: String,
    pub direction: String,
    pub peer: String,
    pub operation: String,
    pub endpoint: String,
    pub model: String,
    pub duration_ms: u128,
    pub usage_source: String,
    pub estimated: bool,
    pub cost_usd: f64,
    pub input_text_tokens: u64,
    pub cached_input_text_tokens: u64,
    pub output_text_tokens: u64,
    pub input_audio_tokens: u64,
    pub output_audio_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Per-model usage and cost totals for a single call.
pub struct ModelUsageSummary {
    pub model: String,
    pub api_call_count: u64,
    pub estimated_api_call_count: u64,
    pub cost_usd: f64,
    #[serde(flatten)]
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Aggregate accounting totals for a call.
pub struct CallAccountingSummary {
    pub api_call_count: u64,
    pub total_cost_usd: f64,
    #[serde(flatten)]
    pub totals: TokenUsage,
    pub model_usage: Vec<ModelUsageSummary>,
}

impl CallAccountingSummary {
    /// Accumulates a new API call entry into the call summary.
    pub fn record(&mut self, entry: &ApiCallLogEntry) {
        self.api_call_count += 1;
        self.total_cost_usd += entry.cost_usd;
        self.totals.merge_from(&TokenUsage {
            input_text_tokens: entry.input_text_tokens,
            cached_input_text_tokens: entry.cached_input_text_tokens,
            output_text_tokens: entry.output_text_tokens,
            input_audio_tokens: entry.input_audio_tokens,
            output_audio_tokens: entry.output_audio_tokens,
        });

        let mut by_model = self
            .model_usage
            .iter()
            .cloned()
            .map(|item| (item.model.clone(), item))
            .collect::<BTreeMap<_, _>>();
        let model = by_model
            .entry(entry.model.clone())
            .or_insert_with(|| ModelUsageSummary {
                model: entry.model.clone(),
                api_call_count: 0,
                estimated_api_call_count: 0,
                cost_usd: 0.0,
                usage: TokenUsage::default(),
            });
        model.api_call_count += 1;
        if entry.estimated {
            model.estimated_api_call_count += 1;
        }
        model.cost_usd += entry.cost_usd;
        model.usage.merge_from(&TokenUsage {
            input_text_tokens: entry.input_text_tokens,
            cached_input_text_tokens: entry.cached_input_text_tokens,
            output_text_tokens: entry.output_text_tokens,
            input_audio_tokens: entry.input_audio_tokens,
            output_audio_tokens: entry.output_audio_tokens,
        });
        self.model_usage = by_model.into_values().collect();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A CSV-friendly rollup for a completed call.
pub struct CallTotalsLogEntry {
    pub ended_at: String,
    pub call_id: String,
    pub direction: String,
    pub peer: String,
    pub started_at: String,
    pub ended_reason: String,
    pub transcript_events: usize,
    pub api_call_count: u64,
    pub total_cost_usd: f64,
    pub input_text_tokens: u64,
    pub cached_input_text_tokens: u64,
    pub output_text_tokens: u64,
    pub input_audio_tokens: u64,
    pub output_audio_tokens: u64,
    pub total_tokens: u64,
    pub model_usage_json: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// The mounted model-pricing catalog used for token cost calculations.
pub struct ModelCatalog {
    #[serde(default)]
    pub defaults: ModelCatalogDefaults,
    pub models: Vec<ModelPricing>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Default estimation settings applied when a model lacks explicit overrides.
pub struct ModelCatalogDefaults {
    #[serde(default = "default_chars_per_token")]
    pub estimated_chars_per_input_token: f64,
    #[serde(default)]
    pub estimated_input_audio_tokens_per_second: Option<f64>,
    #[serde(default)]
    pub estimated_output_audio_tokens_per_second: Option<f64>,
}

impl Default for ModelCatalogDefaults {
    fn default() -> Self {
        Self {
            estimated_chars_per_input_token: default_chars_per_token(),
            estimated_input_audio_tokens_per_second: None,
            estimated_output_audio_tokens_per_second: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Pricing and estimation data for a single model.
pub struct ModelPricing {
    pub name: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub input_text_usd_per_million_tokens: f64,
    #[serde(default)]
    pub cached_input_text_usd_per_million_tokens: f64,
    #[serde(default)]
    pub output_text_usd_per_million_tokens: f64,
    #[serde(default)]
    pub input_audio_usd_per_million_tokens: f64,
    #[serde(default)]
    pub output_audio_usd_per_million_tokens: f64,
    #[serde(default)]
    pub estimated_chars_per_input_token: Option<f64>,
    #[serde(default)]
    pub estimated_input_audio_tokens_per_second: Option<f64>,
    #[serde(default)]
    pub estimated_output_audio_tokens_per_second: Option<f64>,
}

/// Refreshes the on-disk model catalog from the configured pricing page.
pub async fn refresh_model_catalog_from_pricing_page(config: &AccountingConfig) -> Result<()> {
    if !config.refresh_pricing_on_startup {
        return Ok(());
    }

    let response = Client::builder()
        .build()
        .context("failed to build HTTP client for pricing refresh")?
        .get(&config.pricing_page_url)
        .send()
        .await
        .with_context(|| format!("failed to fetch {}", config.pricing_page_url))?
        .error_for_status()
        .with_context(|| {
            format!(
                "pricing page returned an error: {}",
                config.pricing_page_url
            )
        })?;
    let html = response
        .text()
        .await
        .context("failed to read pricing page body")?;
    let scraped = scrape_model_catalog_from_html(&html)?;
    let merged = merge_catalogs(
        load_existing_catalog(&config.model_catalog_path).ok(),
        scraped,
    );
    ensure_parent_dir(&config.model_catalog_path)?;
    fs::write(
        &config.model_catalog_path,
        serde_json::to_vec_pretty(&merged).context("failed to encode refreshed model catalog")?,
    )
    .with_context(|| format!("failed to write {}", config.model_catalog_path))?;
    Ok(())
}

/// Loads model pricing and persists API-call and call-total CSV output.
pub struct AccountingStore {
    catalog: ModelCatalog,
    api_calls_csv_path: String,
    call_totals_csv_path: String,
    write_lock: Mutex<()>,
}

impl AccountingStore {
    /// Loads the accounting store from the configured model catalog.
    pub fn load(config: &AccountingConfig) -> Result<Self> {
        let raw = fs::read_to_string(&config.model_catalog_path).with_context(|| {
            format!("failed to read model catalog {}", config.model_catalog_path)
        })?;
        let catalog: ModelCatalog =
            serde_json::from_str(&raw).context("failed to parse model catalog JSON")?;
        if catalog.models.is_empty() {
            bail!("model catalog did not define any models");
        }
        Ok(Self {
            catalog,
            api_calls_csv_path: config.api_calls_csv_path.clone(),
            call_totals_csv_path: config.call_totals_csv_path.clone(),
            write_lock: Mutex::new(()),
        })
    }

    /// Verifies that all required runtime models exist in the loaded catalog.
    pub fn validate_required_models<'a>(
        &self,
        required_models: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        for model in required_models {
            if self.model_pricing(model).is_none() {
                bail!("model catalog does not define required model {}", model);
            }
        }
        Ok(())
    }

    pub fn estimate_text_tokens(&self, model: &str, text: &str) -> u64 {
        let chars_per_token = self
            .model_pricing(model)
            .and_then(|pricing| pricing.estimated_chars_per_input_token)
            .unwrap_or(self.catalog.defaults.estimated_chars_per_input_token)
            .max(0.1);
        (text.chars().count() as f64 / chars_per_token).ceil() as u64
    }

    pub fn estimate_output_audio_tokens(
        &self,
        model: &str,
        sample_count: usize,
        sample_rate: u32,
    ) -> u64 {
        let Some(tokens_per_second) = self
            .model_pricing(model)
            .and_then(|pricing| pricing.estimated_output_audio_tokens_per_second)
            .or(self
                .catalog
                .defaults
                .estimated_output_audio_tokens_per_second)
        else {
            return 0;
        };
        ((sample_count as f64 / sample_rate as f64) * tokens_per_second).ceil() as u64
    }

    /// Resolve a potentially untrusted log path relative to the current directory
    /// and ensure it does not escape that base directory.
    fn safe_log_path<P: AsRef<Path>>(&self, path: P) -> Result<PathBuf> {
        let base_dir = env::current_dir().context("failed to get current working directory")?;
        let candidate = base_dir.join(path.as_ref());
        let canonical = candidate
            .canonicalize()
            .or_else(|_| {
                // If the file does not exist yet, ensure parent exists within base_dir,
                // and use the joined path without canonicalization for the safety check.
                Ok(candidate.clone())
            })
            .with_context(|| format!("failed to resolve log path {:?}", candidate))?;
        if !canonical.starts_with(&base_dir) {
            bail!("log path escapes base directory");
        }
        Ok(canonical)
    }

    pub fn record_api_call(
        &self,
        context: ApiCallContext<'_>,
        usage: TokenUsage,
    ) -> Result<ApiCallLogEntry> {
        let entry = ApiCallLogEntry {
            at: context.at.to_string(),
            call_id: context.call_id.to_string(),
            direction: context.direction.to_string(),
            peer: context.peer.to_string(),
            operation: context.operation.to_string(),
            endpoint: context.endpoint.to_string(),
            model: context.model.to_string(),
            duration_ms: context.duration_ms,
            usage_source: context.usage_source.to_string(),
            estimated: context.estimated,
            cost_usd: self.compute_cost(context.model, &usage),
            input_text_tokens: usage.input_text_tokens,
            cached_input_text_tokens: usage.cached_input_text_tokens,
            output_text_tokens: usage.output_text_tokens,
            input_audio_tokens: usage.input_audio_tokens,
            output_audio_tokens: usage.output_audio_tokens,
            total_tokens: usage.total_tokens(),
        };
        self.append_api_call_csv(&entry)?;
        Ok(entry)
    }

    pub fn record_call_total(&self, entry: &CallTotalsLogEntry) -> Result<()> {
        let _guard = self.write_lock.lock();
        let call_totals_path = self.safe_log_path(&self.call_totals_csv_path)?;
        let call_totals_path_str = call_totals_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("call_totals_path is not valid UTF-8"))?;
        ensure_parent_dir(call_totals_path_str)?;
        let file_exists = Path::new(&call_totals_path).exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&call_totals_path)
            .with_context(|| format!("failed to open {}", call_totals_path.display()))?;
        if !file_exists {
            file.write_all(CALL_TOTALS_HEADER.as_bytes())
                .context("failed to write call totals CSV header")?;
        }
        file.write_all(
            csv_row(&[
                entry.ended_at.clone(),
                entry.call_id.clone(),
                entry.direction.clone(),
                entry.peer.clone(),
                entry.started_at.clone(),
                entry.ended_reason.clone(),
                entry.transcript_events.to_string(),
                entry.api_call_count.to_string(),
                format_usd(entry.total_cost_usd),
                entry.input_text_tokens.to_string(),
                entry.cached_input_text_tokens.to_string(),
                entry.output_text_tokens.to_string(),
                entry.input_audio_tokens.to_string(),
                entry.output_audio_tokens.to_string(),
                entry.total_tokens.to_string(),
                entry.model_usage_json.clone(),
            ])
            .as_bytes(),
        )
        .context("failed to append call totals CSV row")?;
        Ok(())
    }

    fn append_api_call_csv(&self, entry: &ApiCallLogEntry) -> Result<()> {
        let _guard = self.write_lock.lock();
        let api_calls_path = self.safe_log_path(&self.api_calls_csv_path)?;
        ensure_parent_dir(&api_calls_path)?;
        let file_exists = Path::new(&api_calls_path).exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&api_calls_path)
            .with_context(|| format!("failed to open {}", api_calls_path.display()))?;
        if !file_exists {
            file.write_all(API_CALLS_HEADER.as_bytes())
                .context("failed to write API calls CSV header")?;
        }
        file.write_all(
            csv_row(&[
                entry.at.clone(),
                entry.call_id.clone(),
                entry.direction.clone(),
                entry.peer.clone(),
                entry.operation.clone(),
                entry.endpoint.clone(),
                entry.model.clone(),
                entry.duration_ms.to_string(),
                entry.usage_source.clone(),
                entry.estimated.to_string(),
                format_usd(entry.cost_usd),
                entry.input_text_tokens.to_string(),
                entry.cached_input_text_tokens.to_string(),
                entry.output_text_tokens.to_string(),
                entry.input_audio_tokens.to_string(),
                entry.output_audio_tokens.to_string(),
                entry.total_tokens.to_string(),
            ])
            .as_bytes(),
        )
        .context("failed to append API calls CSV row")?;
        Ok(())
    }

    fn compute_cost(&self, model: &str, usage: &TokenUsage) -> f64 {
        let Some(pricing) = self.model_pricing(model) else {
            return 0.0;
        };
        let uncached_input_text = usage
            .input_text_tokens
            .saturating_sub(usage.cached_input_text_tokens);
        (uncached_input_text as f64 * pricing.input_text_usd_per_million_tokens / 1_000_000.0)
            + (usage.cached_input_text_tokens as f64
                * pricing.cached_input_text_usd_per_million_tokens
                / 1_000_000.0)
            + (usage.output_text_tokens as f64 * pricing.output_text_usd_per_million_tokens
                / 1_000_000.0)
            + (usage.input_audio_tokens as f64 * pricing.input_audio_usd_per_million_tokens
                / 1_000_000.0)
            + (usage.output_audio_tokens as f64 * pricing.output_audio_usd_per_million_tokens
                / 1_000_000.0)
    }

    fn model_pricing(&self, model: &str) -> Option<&ModelPricing> {
        self.catalog
            .models
            .iter()
            .find(|candidate| candidate.name == model)
    }
}

fn load_existing_catalog(path: &str) -> Result<ModelCatalog> {
    let raw = fs::read_to_string(path).with_context(|| format!("failed to read {}", path))?;
    serde_json::from_str(&raw).context("failed to parse existing model catalog")
}

fn scrape_model_catalog_from_html(html: &str) -> Result<ModelCatalog> {
    let document = Html::parse_document(html);
    let lines = document
        .root_element()
        .text()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(normalize_whitespace)
        .collect::<Vec<_>>();

    let mut models = BTreeMap::<String, ModelPricing>::new();
    let mut in_transcription_section = false;
    let mut token_section = "";
    let mut idx = 0usize;
    while idx < lines.len() {
        let line = &lines[idx];
        match line.as_str() {
            "Transcription and speech generation" => {
                in_transcription_section = true;
                token_section = "";
            }
            "Image tokens" | "Image generation" | "Built-in tools" | "Web search"
                if in_transcription_section =>
            {
                in_transcription_section = false;
                token_section = "";
            }
            "Text tokens" => token_section = "text",
            "Audio tokens" => token_section = "audio",
            _ => {}
        }

        if in_transcription_section
            && looks_like_model_name(line)
            && idx + 2 < lines.len()
            && (is_price_token(&lines[idx + 1]) || lines[idx + 1] == "*")
            && (is_price_token(&lines[idx + 2]) || lines[idx + 2] == "*")
        {
            let model = models.entry(line.clone()).or_insert_with(|| ModelPricing {
                name: line.clone(),
                service: if line.contains("tts") {
                    "tts".to_string()
                } else {
                    "transcription".to_string()
                },
                input_text_usd_per_million_tokens: 0.0,
                cached_input_text_usd_per_million_tokens: 0.0,
                output_text_usd_per_million_tokens: 0.0,
                input_audio_usd_per_million_tokens: 0.0,
                output_audio_usd_per_million_tokens: 0.0,
                estimated_chars_per_input_token: None,
                estimated_input_audio_tokens_per_second: None,
                estimated_output_audio_tokens_per_second: None,
            });
            let input_price = parse_price(&lines[idx + 1]).unwrap_or(0.0);
            let output_price = parse_price(&lines[idx + 2]).unwrap_or(0.0);
            if token_section == "text" {
                model.input_text_usd_per_million_tokens = input_price;
                model.output_text_usd_per_million_tokens = output_price;
            } else if token_section == "audio" {
                model.input_audio_usd_per_million_tokens = input_price;
                model.output_audio_usd_per_million_tokens = output_price;
            }
            idx += 4;
            continue;
        }

        if token_section == "text"
            && !in_transcription_section
            && let Some((model_name, prices)) = parse_concatenated_pricing_row(line)
            && prices.len() >= 3
        {
            let model = models
                .entry(model_name.clone())
                .or_insert_with(|| ModelPricing {
                    name: model_name.clone(),
                    service: "responses".to_string(),
                    input_text_usd_per_million_tokens: 0.0,
                    cached_input_text_usd_per_million_tokens: 0.0,
                    output_text_usd_per_million_tokens: 0.0,
                    input_audio_usd_per_million_tokens: 0.0,
                    output_audio_usd_per_million_tokens: 0.0,
                    estimated_chars_per_input_token: None,
                    estimated_input_audio_tokens_per_second: None,
                    estimated_output_audio_tokens_per_second: None,
                });
            model.input_text_usd_per_million_tokens = prices[0].unwrap_or(0.0);
            model.cached_input_text_usd_per_million_tokens = prices[1].unwrap_or(0.0);
            model.output_text_usd_per_million_tokens = prices[2].unwrap_or(0.0);
        }

        idx += 1;
    }

    if models.is_empty() {
        bail!("pricing page scrape did not yield any models");
    }

    Ok(ModelCatalog {
        defaults: ModelCatalogDefaults::default(),
        models: models.into_values().collect(),
    })
}

fn merge_catalogs(existing: Option<ModelCatalog>, scraped: ModelCatalog) -> ModelCatalog {
    let defaults = existing
        .as_ref()
        .map(|catalog| catalog.defaults.clone())
        .unwrap_or_default();
    let mut merged = existing
        .map(|catalog| {
            catalog
                .models
                .into_iter()
                .map(|model| (model.name.clone(), model))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    for scraped_model in scraped.models {
        merged
            .entry(scraped_model.name.clone())
            .and_modify(|existing_model| {
                existing_model.service = scraped_model.service.clone();
                existing_model.input_text_usd_per_million_tokens =
                    scraped_model.input_text_usd_per_million_tokens;
                existing_model.cached_input_text_usd_per_million_tokens =
                    scraped_model.cached_input_text_usd_per_million_tokens;
                existing_model.output_text_usd_per_million_tokens =
                    scraped_model.output_text_usd_per_million_tokens;
                existing_model.input_audio_usd_per_million_tokens =
                    scraped_model.input_audio_usd_per_million_tokens;
                existing_model.output_audio_usd_per_million_tokens =
                    scraped_model.output_audio_usd_per_million_tokens;
            })
            .or_insert(scraped_model);
    }

    ModelCatalog {
        defaults,
        models: merged.into_values().collect(),
    }
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_model_name(value: &str) -> bool {
    value.contains("gpt")
        || value.starts_with("o1")
        || value.starts_with("o3")
        || value.starts_with("o4")
}

fn is_price_token(value: &str) -> bool {
    value.starts_with('$') || value == "*"
}

fn parse_price(value: &str) -> Option<f64> {
    value
        .strip_prefix('$')
        .unwrap_or(value)
        .split_whitespace()
        .next()
        .map(|token| token.trim_end_matches('/'))
        .and_then(|token| token.parse::<f64>().ok())
}

fn parse_concatenated_pricing_row(line: &str) -> Option<(String, Vec<Option<f64>>)> {
    let first_dollar = line.find('$')?;
    let model = line[..first_dollar].trim();
    if !looks_like_model_name(model) {
        return None;
    }

    let mut prices = Vec::new();
    let bytes = line.as_bytes();
    let mut idx = first_dollar;
    while idx < bytes.len() {
        match bytes[idx] {
            b'$' => {
                let start = idx + 1;
                idx += 1;
                while idx < bytes.len() && (bytes[idx].is_ascii_digit() || bytes[idx] == b'.') {
                    idx += 1;
                }
                prices.push(line[start..idx].parse::<f64>().ok());
            }
            b'-' => {
                prices.push(None);
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    Some((model.to_string(), prices))
}

pub struct ApiCallContext<'a> {
    pub at: &'a str,
    pub call_id: &'a str,
    pub direction: &'a str,
    pub peer: &'a str,
    pub operation: &'a str,
    pub endpoint: &'a str,
    pub model: &'a str,
    pub duration_ms: u128,
    pub usage_source: &'a str,
    pub estimated: bool,
}

const API_CALLS_HEADER: &str = "at,call_id,direction,peer,operation,endpoint,model,duration_ms,usage_source,estimated,cost_usd,input_text_tokens,cached_input_text_tokens,output_text_tokens,input_audio_tokens,output_audio_tokens,total_tokens\n";
const CALL_TOTALS_HEADER: &str = "ended_at,call_id,direction,peer,started_at,ended_reason,transcript_events,api_call_count,total_cost_usd,input_text_tokens,cached_input_text_tokens,output_text_tokens,input_audio_tokens,output_audio_tokens,total_tokens,model_usage_json\n";

const fn default_chars_per_token() -> f64 {
    4.0
}

fn ensure_parent_dir(path: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    Ok(())
}

fn csv_row(columns: &[String]) -> String {
    let escaped = columns
        .iter()
        .map(|value| {
            if value.contains([',', '"', '\n']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>();
    format!("{}\n", escaped.join(","))
}

fn format_usd(value: f64) -> String {
    format!("{value:.8}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_summary_aggregates_model_usage() {
        let mut summary = CallAccountingSummary::default();
        summary.record(&ApiCallLogEntry {
            at: "2026-03-15T00:00:00Z".to_string(),
            call_id: "abc".to_string(),
            direction: "inbound".to_string(),
            peer: "1001".to_string(),
            operation: "response".to_string(),
            endpoint: "https://example.com".to_string(),
            model: "gpt-4o-mini".to_string(),
            duration_ms: 123,
            usage_source: "api".to_string(),
            estimated: false,
            cost_usd: 0.001,
            input_text_tokens: 100,
            cached_input_text_tokens: 20,
            output_text_tokens: 40,
            input_audio_tokens: 0,
            output_audio_tokens: 0,
            total_tokens: 140,
        });
        summary.record(&ApiCallLogEntry {
            at: "2026-03-15T00:00:02Z".to_string(),
            call_id: "abc".to_string(),
            direction: "inbound".to_string(),
            peer: "1001".to_string(),
            operation: "tts".to_string(),
            endpoint: "https://example.com".to_string(),
            model: "gpt-4o-mini-tts".to_string(),
            duration_ms: 321,
            usage_source: "estimated".to_string(),
            estimated: true,
            cost_usd: 0.002,
            input_text_tokens: 50,
            cached_input_text_tokens: 0,
            output_text_tokens: 0,
            input_audio_tokens: 0,
            output_audio_tokens: 25,
            total_tokens: 75,
        });

        assert_eq!(summary.api_call_count, 2);
        assert_eq!(summary.totals.input_text_tokens, 150);
        assert_eq!(summary.totals.output_audio_tokens, 25);
        assert_eq!(summary.model_usage.len(), 2);
    }

    #[test]
    fn compute_cost_uses_cached_input_rate() {
        let store = AccountingStore {
            catalog: ModelCatalog {
                defaults: ModelCatalogDefaults::default(),
                models: vec![ModelPricing {
                    name: "gpt-4o-mini".to_string(),
                    service: "responses".to_string(),
                    input_text_usd_per_million_tokens: 0.15,
                    cached_input_text_usd_per_million_tokens: 0.075,
                    output_text_usd_per_million_tokens: 0.60,
                    input_audio_usd_per_million_tokens: 0.0,
                    output_audio_usd_per_million_tokens: 0.0,
                    estimated_chars_per_input_token: None,
                    estimated_input_audio_tokens_per_second: None,
                    estimated_output_audio_tokens_per_second: None,
                }],
            },
            api_calls_csv_path: "/tmp/api_calls.csv".to_string(),
            call_totals_csv_path: "/tmp/call_totals.csv".to_string(),
            write_lock: Mutex::new(()),
        };
        let cost = store.compute_cost(
            "gpt-4o-mini",
            &TokenUsage {
                input_text_tokens: 1000,
                cached_input_text_tokens: 200,
                output_text_tokens: 500,
                input_audio_tokens: 0,
                output_audio_tokens: 0,
            },
        );

        assert!((cost - 0.000435).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_text_tokens_uses_catalog_override() {
        let store = AccountingStore {
            catalog: ModelCatalog {
                defaults: ModelCatalogDefaults::default(),
                models: vec![ModelPricing {
                    name: "gpt-4o-mini-tts".to_string(),
                    service: "tts".to_string(),
                    input_text_usd_per_million_tokens: 0.60,
                    cached_input_text_usd_per_million_tokens: 0.0,
                    output_text_usd_per_million_tokens: 0.0,
                    input_audio_usd_per_million_tokens: 0.0,
                    output_audio_usd_per_million_tokens: 12.0,
                    estimated_chars_per_input_token: Some(2.0),
                    estimated_input_audio_tokens_per_second: None,
                    estimated_output_audio_tokens_per_second: Some(20.0),
                }],
            },
            api_calls_csv_path: "/tmp/api_calls.csv".to_string(),
            call_totals_csv_path: "/tmp/call_totals.csv".to_string(),
            write_lock: Mutex::new(()),
        };

        assert_eq!(store.estimate_text_tokens("gpt-4o-mini-tts", "hello"), 3);
        assert_eq!(
            store.estimate_output_audio_tokens("gpt-4o-mini-tts", 8000, 8000),
            20
        );
    }

    #[test]
    fn scrape_model_catalog_parses_text_and_audio_sections() {
        let html = r#"
        <html>
          <body>
            <h2>Text tokens</h2>
            <p>Model Input Cached Input Output</p>
            <p>gpt-4o-mini $0.15 $0.075 $0.60</p>
            <h2>Transcription and speech generation</h2>
            <h3>Text tokens</h3>
            <p>gpt-4o-transcribe</p>
            <p>$2.40</p>
            <p>$9.60</p>
            <p>$0.006 / minute</p>
            <p>gpt-4o-mini-tts</p>
            <p>$0.60</p>
            <p>*</p>
            <p>$0.015 / minute</p>
            <h3>Audio tokens</h3>
            <p>gpt-4o-transcribe</p>
            <p>$6.00</p>
            <p>*</p>
            <p>$0.006 / minute</p>
            <p>gpt-4o-mini-tts</p>
            <p>*</p>
            <p>$12.00</p>
            <p>$0.015 / minute</p>
          </body>
        </html>
        "#;

        let catalog = scrape_model_catalog_from_html(html).expect("catalog");
        let text_model = catalog
            .models
            .iter()
            .find(|model| model.name == "gpt-4o-mini")
            .expect("gpt-4o-mini");
        assert_eq!(text_model.input_text_usd_per_million_tokens, 0.15);
        assert_eq!(text_model.cached_input_text_usd_per_million_tokens, 0.075);
        assert_eq!(text_model.output_text_usd_per_million_tokens, 0.60);

        let transcribe = catalog
            .models
            .iter()
            .find(|model| model.name == "gpt-4o-transcribe")
            .expect("gpt-4o-transcribe");
        assert_eq!(transcribe.input_text_usd_per_million_tokens, 2.40);
        assert_eq!(transcribe.output_text_usd_per_million_tokens, 9.60);
        assert_eq!(transcribe.input_audio_usd_per_million_tokens, 6.00);

        let tts = catalog
            .models
            .iter()
            .find(|model| model.name == "gpt-4o-mini-tts")
            .expect("gpt-4o-mini-tts");
        assert_eq!(tts.input_text_usd_per_million_tokens, 0.60);
        assert_eq!(tts.output_audio_usd_per_million_tokens, 12.00);
    }
}
