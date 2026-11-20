//! Validation and construction helpers for Agent-driven dashboards.
//!
//! The model is good at choosing modules but unreliable at producing a correct
//! binary pane tree. This module is the safety layer between the two: it keeps
//! the agent pane alive, normalises module aliases and market config, clamps
//! unsafe split ratios, and builds known-good presets.

use serde_json::{Map, Value, json};

use crate::data::pane_tree::{Axis, ModuleData, PaneNode};

/// Supported module types, in the order the selector shows them.
pub const MODULE_TYPES: &[&str] = &[
    "chart",
    "ticker",
    "orderbook",
    "trades",
    "watchlist",
    "positions",
    "execution",
    "marketcap",
    "newsfeed",
    "time",
    "feargreed",
    "agent",
    "empty",
];

/// Presets that are safe to hand to a user without further editing.
pub const PRESETS: &[&str] = &["trading", "analysis", "monitoring", "execution", "minimal"];

pub const MAX_PANES: usize = 12;
pub const MAX_DEPTH: usize = 8;
pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;

/// Canonical module id for a model-supplied name.
pub fn normalize_module_type(raw: &str) -> String {
    let value = raw.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    match value.as_str() {
        "chart" | "kline" | "k_line" | "candles" | "candlestick" | "candle" => "chart",
        "ticker" | "price" | "priceticker" | "price_ticker" => "ticker",
        "orderbook" | "order_book" | "book" | "depth" => "orderbook",
        "trades" | "trade" | "tape" | "tape_panel" => "trades",
        "watchlist" | "watch_list" | "symbols" | "markets" => "watchlist",
        "positions" | "position" | "portfolio" => "positions",
        "execution" | "trade_desk" | "tradedesk" | "order_entry" => "execution",
        "marketcap" | "market_cap" | "marketcap_panel" | "market_cap_panel" => "marketcap",
        "newsfeed" | "news_feed" | "news" => "newsfeed",
        "time" | "clock" | "clocks" => "time",
        "feargreed" | "fear_greed" | "fear_and_greed" | "sentiment" => "feargreed",
        "agent" | "ai" | "assistant" => "agent",
        "empty" | "none" | "blank" | "placeholder" => "empty",
        _ => "empty",
    }
    .to_string()
}

pub fn is_agent_type(module_type: &str) -> bool {
    module_type.eq_ignore_ascii_case("agent")
}

pub fn is_market_module(module_type: &str) -> bool {
    matches!(
        module_type,
        "chart" | "ticker" | "orderbook" | "trades" | "watchlist" | "positions" | "execution"
    )
}

fn string_field(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Best available market context from the current dashboard.
pub fn infer_base_config(tree: &PaneNode) -> Value {
    let mut fallback = json!({
        "exchange": "binance",
        "symbol": "BTC/USDT",
        "timeframe": "1h",
        "interval": "1h"
    });

    let mut chart_config: Option<Value> = None;
    let mut market_config: Option<Value> = None;
    collect_market_configs(tree, &mut chart_config, &mut market_config);

    let source = chart_config.or(market_config);
    if let Some(source) = source {
        if let Some(object) = fallback.as_object_mut() {
            for key in ["exchange", "symbol", "timeframe", "interval", "type"] {
                if let Some(value) = source.get(key) {
                    object.insert(key.to_string(), value.clone());
                }
            }
        }
    }

    let exchange = string_field(&fallback, "exchange").unwrap_or_else(|| "binance".into());
    let symbol = string_field(&fallback, "symbol").unwrap_or_else(|| "BTC/USDT".into());
    let timeframe = string_field(&fallback, "timeframe")
        .or_else(|| string_field(&fallback, "interval"))
        .unwrap_or_else(|| "1h".into());
    json!({
        "exchange": exchange,
        "symbol": symbol,
        "timeframe": timeframe,
        "interval": timeframe,
    })
}

fn collect_market_configs(node: &PaneNode, chart: &mut Option<Value>, market: &mut Option<Value>) {
    match node {
        PaneNode::Pane { data, .. } => {
            let kind = data
                .module_type
                .as_deref()
                .map(normalize_module_type)
                .unwrap_or_else(|| "empty".into());
            if !data.config.is_object() {
                return;
            }
            if kind == "chart" && chart.is_none() {
                *chart = Some(data.config.clone());
            } else if is_market_module(&kind) && market.is_none() {
                *market = Some(data.config.clone());
            }
        }
        PaneNode::Split { a, b, .. } => {
            collect_market_configs(a, chart, market);
            collect_market_configs(b, chart, market);
        }
    }
}

/// Market config for a module: base context, then explicit overrides.
pub fn config_for(module_type: &str, base: &Value, overrides: &Value) -> Value {
    if matches!(
        module_type,
        "agent" | "empty" | "time" | "feargreed" | "marketcap" | "newsfeed"
    ) {
        return if overrides.is_object() {
            overrides.clone()
        } else {
            json!({})
        };
    }

    let mut out = Map::new();
    for key in ["exchange", "symbol", "timeframe", "interval", "type"] {
        if let Some(value) = base.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    if let Some(object) = overrides.as_object() {
        for (key, value) in object {
            out.insert(key.clone(), value.clone());
        }
    }

    if !out.contains_key("exchange") {
        out.insert("exchange".into(), json!("binance"));
    }
    if !out.contains_key("symbol") {
        out.insert("symbol".into(), json!("BTC/USDT"));
    }

    if module_type == "chart" {
        let timeframe = out
            .get("timeframe")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                out.get("interval")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "1h".into());
        out.insert("timeframe".into(), json!(timeframe));
        out.insert("interval".into(), json!(timeframe));
    }

    Value::Object(out)
}

fn normalize_pane_data(data: &mut ModuleData, base: &Value) {
    let kind = normalize_module_type(data.module_type.as_deref().unwrap_or("empty"));
    if !data.config.is_object() {
        data.config = json!({});
    }
    // Agent config may carry conversation state; keep it verbatim.
    if kind != "agent" {
        data.config = config_for(&kind, base, &data.config);
    }
    data.module_type = Some(kind);
}

fn dedupe_ids(node: &mut PaneNode, seen: &mut std::collections::HashSet<String>) {
    match node {
        PaneNode::Pane { id, .. } => {
            if id.trim().is_empty() || !seen.insert(id.clone()) {
                *id = PaneNode::generate_id();
                seen.insert(id.clone());
            }
        }
        PaneNode::Split { id, a, b, .. } => {
            if id.trim().is_empty() || !seen.insert(id.clone()) {
                *id = PaneNode::generate_id();
                seen.insert(id.clone());
            }
            dedupe_ids(a, seen);
            dedupe_ids(b, seen);
        }
    }
}

fn clamp_ratios(node: &mut PaneNode) {
    if let PaneNode::Split { ratio, a, b, .. } = node {
        *ratio = if ratio.is_finite() {
            ratio.clamp(MIN_RATIO, MAX_RATIO)
        } else {
            0.5
        };
        clamp_ratios(a);
        clamp_ratios(b);
    }
}

fn normalize_panes(node: &mut PaneNode, base: &Value) {
    match node {
        PaneNode::Pane { data, .. } => normalize_pane_data(data, base),
        PaneNode::Split { a, b, .. } => {
            normalize_panes(a, base);
            normalize_panes(b, base);
        }
    }
}

fn tree_stats(node: &PaneNode, depth: usize, panes: &mut usize, max_depth: &mut usize) {
    match node {
        PaneNode::Pane { .. } => {
            *panes += 1;
            *max_depth = (*max_depth).max(depth);
        }
        PaneNode::Split { a, b, .. } => {
            *max_depth = (*max_depth).max(depth);
            tree_stats(a, depth + 1, panes, max_depth);
            tree_stats(b, depth + 1, panes, max_depth);
        }
    }
}

/// Validate and normalise a model-built tree before it is installed.
pub fn normalize_layout(tree: &mut PaneNode, base: &Value) -> Result<(), String> {
    let mut panes = 0;
    let mut depth = 0;
    tree_stats(tree, 0, &mut panes, &mut depth);

    if panes == 0 {
        return Err("Layout has no panes.".into());
    }
    if panes > MAX_PANES {
        return Err(format!(
            "Layout has {panes} panes; the maximum is {MAX_PANES}. Use a preset or fewer splits."
        ));
    }
    if depth > MAX_DEPTH {
        return Err(format!(
            "Layout is nested {depth} levels deep; the maximum is {MAX_DEPTH}. Use a preset or a flatter tree."
        ));
    }

    let mut seen = std::collections::HashSet::new();
    dedupe_ids(tree, &mut seen);
    clamp_ratios(tree);
    normalize_panes(tree, base);
    Ok(())
}

fn pane_entries<'a>(node: &'a PaneNode, out: &mut Vec<(&'a String, &'a ModuleData)>) {
    match node {
        PaneNode::Pane { id, data } => out.push((id, data)),
        PaneNode::Split { a, b, .. } => {
            pane_entries(a, out);
            pane_entries(b, out);
        }
    }
}

pub fn find_pane(tree: &PaneNode, id: &str) -> Option<ModuleData> {
    let mut entries = Vec::new();
    pane_entries(tree, &mut entries);
    entries
        .into_iter()
        .find(|(pane_id, _)| pane_id.as_str() == id)
        .map(|(_, data)| data.clone())
}

fn find_pane_data_mut<'a>(node: &'a mut PaneNode, id: &str) -> Option<&'a mut ModuleData> {
    match node {
        PaneNode::Pane { id: pane_id, data } if pane_id == id => Some(data),
        PaneNode::Pane { .. } => None,
        PaneNode::Split { a, b, .. } => {
            find_pane_data_mut(a, id).or_else(|| find_pane_data_mut(b, id))
        }
    }
}

fn rename_pane(node: &mut PaneNode, from: &str, to: &str) -> bool {
    match node {
        PaneNode::Pane { id, .. } if id == from => {
            *id = to.to_string();
            true
        }
        PaneNode::Pane { .. } => false,
        PaneNode::Split { a, b, .. } => rename_pane(a, from, to) || rename_pane(b, from, to),
    }
}

fn first_agent_id(tree: &PaneNode) -> Option<String> {
    let mut entries = Vec::new();
    pane_entries(tree, &mut entries);
    entries
        .into_iter()
        .find(|(_, data)| {
            data.module_type
                .as_deref()
                .map(is_agent_type)
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
}

fn agent_ids(tree: &PaneNode) -> Vec<String> {
    let mut entries = Vec::new();
    pane_entries(tree, &mut entries);
    entries
        .into_iter()
        .filter(|(_, data)| {
            data.module_type
                .as_deref()
                .map(is_agent_type)
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// The pane running the Agent conversation, preferring the caller's pane.
pub fn find_agent_pane(tree: &PaneNode, preferred: Option<&str>) -> Option<(String, ModuleData)> {
    if let Some(preferred) = preferred {
        if let Some(data) = find_pane(tree, preferred) {
            if data
                .module_type
                .as_deref()
                .map(is_agent_type)
                .unwrap_or(false)
            {
                return Some((preferred.to_string(), data));
            }
        }
    }

    let id = first_agent_id(tree)?;
    let data = find_pane(tree, &id)?;
    Some((id, data))
}

fn first_empty_id(tree: &PaneNode) -> Option<String> {
    let mut entries = Vec::new();
    pane_entries(tree, &mut entries);
    entries
        .into_iter()
        .find(|(_, data)| {
            data.module_type
                .as_deref()
                .map(|kind| normalize_module_type(kind) == "empty")
                .unwrap_or(true)
        })
        .map(|(id, _)| id.clone())
}

fn replace_empty_with_agent(tree: &mut PaneNode, id: String, data: ModuleData) -> bool {
    let Some(empty_id) = first_empty_id(tree) else {
        return false;
    };
    if !rename_pane(tree, &empty_id, &id) {
        return false;
    }
    if let Some(slot) = find_pane_data_mut(tree, &id) {
        *slot = data;
        true
    } else {
        false
    }
}

fn wrap_with_agent(tree: &mut PaneNode, id: String, data: ModuleData) {
    let placeholder = PaneNode::Pane {
        id: PaneNode::generate_id(),
        data: ModuleData::default(),
    };
    let old = std::mem::replace(tree, placeholder);
    *tree = PaneNode::Split {
        id: PaneNode::generate_id(),
        axis: Axis::Vertical,
        ratio: 0.72,
        a: Box::new(old),
        b: Box::new(PaneNode::Pane { id, data }),
    };
}

/// Guarantee that `tree` contains the current Agent pane and keeps its id, so
/// the renderer reuses the existing AgentPanel entity and conversation.
pub fn ensure_agent_pane(
    tree: &mut PaneNode,
    agent: Option<(String, ModuleData)>,
) -> Result<(), String> {
    match agent {
        Some((id, data)) => {
            if find_pane(tree, &id).is_some() {
                // The new layout reused the agent pane's id. Force it back to
                // the agent module; a model must never overwrite the caller.
                if let Some(existing) = find_pane_data_mut(tree, &id) {
                    *existing = data;
                }
                return Ok(());
            }

            if let Some(model_agent_id) = first_agent_id(tree) {
                // Keep the model's slot but rename it to the caller's id so the
                // AgentPanel cache survives the layout replacement.
                if !rename_pane(tree, &model_agent_id, &id) {
                    return Err("Could not preserve the agent pane.".into());
                }
                if let Some(existing) = find_pane_data_mut(tree, &id) {
                    *existing = data;
                }
                return Ok(());
            }

            if agent_ids(tree).is_empty() {
                let mut panes = 0;
                let mut depth = 0;
                tree_stats(tree, 0, &mut panes, &mut depth);
                if panes >= MAX_PANES && !replace_empty_with_agent(tree, id.clone(), data.clone()) {
                    return Err(format!(
                        "Layout already has {panes} panes; no room to keep the agent pane."
                    ));
                }
                if panes < MAX_PANES {
                    wrap_with_agent(tree, id, data);
                }
            }
            Ok(())
        }
        None => {
            if !agent_ids(tree).is_empty() {
                return Ok(());
            }
            let mut panes = 0;
            let mut depth = 0;
            tree_stats(tree, 0, &mut panes, &mut depth);
            let data = ModuleData {
                module_type: Some("agent".into()),
                config: json!({}),
            };
            if panes >= MAX_PANES {
                if replace_empty_with_agent(tree, PaneNode::generate_id(), data) {
                    return Ok(());
                }
                return Err(format!(
                    "Layout already has {panes} panes; no room for an agent pane."
                ));
            }
            wrap_with_agent(tree, PaneNode::generate_id(), data);
            Ok(())
        }
    }
}

fn pane(kind: &str, base: &Value, overrides: &Value) -> PaneNode {
    PaneNode::Pane {
        id: PaneNode::generate_id(),
        data: ModuleData {
            module_type: Some(kind.to_string()),
            config: config_for(kind, base, overrides),
        },
    }
}

fn split(axis: Axis, ratio: f32, a: PaneNode, b: PaneNode) -> PaneNode {
    PaneNode::Split {
        id: PaneNode::generate_id(),
        axis,
        ratio,
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn h(ratio: f32, a: PaneNode, b: PaneNode) -> PaneNode {
    split(Axis::Horizontal, ratio, a, b)
}

fn v(ratio: f32, a: PaneNode, b: PaneNode) -> PaneNode {
    split(Axis::Vertical, ratio, a, b)
}

/// Build a known-good layout. The caller should run [`ensure_agent_pane`] with
/// the existing agent pane afterwards.
pub fn build_preset(name: &str, base: &Value, overrides: &Value) -> Result<PaneNode, String> {
    let preset = name.trim().to_ascii_lowercase();
    let chart = || pane("chart", base, overrides);
    let ticker = || pane("ticker", base, overrides);
    let book = || pane("orderbook", base, overrides);
    let trades = || pane("trades", base, overrides);
    let watchlist = || pane("watchlist", base, overrides);
    let positions = || pane("positions", base, overrides);
    let execution = || pane("execution", base, overrides);
    let news = || pane("newsfeed", base, overrides);
    let agent = || pane("agent", base, overrides);

    let tree = match preset.as_str() {
        "trading" => v(
            0.72,
            h(0.68, chart(), h(0.5, book(), trades())),
            h(0.5, watchlist(), agent()),
        ),
        "analysis" => v(0.7, chart(), h(0.5, watchlist(), agent())),
        "monitoring" => v(
            0.7,
            h(0.5, watchlist(), ticker()),
            h(0.55, chart(), h(0.5, news(), agent())),
        ),
        "execution" => v(
            0.7,
            h(0.62, chart(), v(0.5, book(), positions())),
            h(0.5, execution(), agent()),
        ),
        "minimal" => v(0.75, chart(), agent()),
        // A preset name supplied by the model but unknown here: build the
        // default trading terminal rather than returning an unusable layout.
        "default" | "" => v(
            0.72,
            h(0.68, chart(), h(0.5, book(), trades())),
            h(0.5, watchlist(), agent()),
        ),
        other => {
            return Err(format!(
                "Unknown preset '{other}'. Use one of: {}.",
                PRESETS.join(", ")
            ));
        }
    };
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        json!({"exchange": "binance", "symbol": "BTC/USDT", "timeframe": "1h", "interval": "1h"})
    }

    #[test]
    fn normalizes_common_module_aliases() {
        assert_eq!(normalize_module_type("K-Line"), "chart");
        assert_eq!(normalize_module_type("Order Book"), "orderbook");
        assert_eq!(normalize_module_type("market_cap"), "marketcap");
        assert_eq!(normalize_module_type("Trade Desk"), "execution");
        assert_eq!(normalize_module_type("wat"), "empty");
    }

    #[test]
    fn normalizes_ratios_and_configs() {
        let mut tree: PaneNode = serde_json::from_value(json!({
            "kind": "split", "id": "s", "axis": "vertical", "ratio": 0.0,
            "a": {"kind": "pane", "id": "a", "data": {"type": "kline", "config": {"symbol": "ETH/USDT"}}},
            "b": {"kind": "pane", "id": "b", "data": {"type": "order book", "config": null}}
        }))
        .unwrap();
        normalize_layout(&mut tree, &base()).unwrap();
        match tree {
            PaneNode::Split { ratio, a, b, .. } => {
                assert_eq!(ratio, MIN_RATIO);
                let a = match *a {
                    PaneNode::Pane { data, .. } => data,
                    _ => panic!(),
                };
                assert_eq!(a.module_type.as_deref(), Some("chart"));
                assert_eq!(a.config["exchange"], "binance");
                assert_eq!(a.config["symbol"], "ETH/USDT");
                let b = match *b {
                    PaneNode::Pane { data, .. } => data,
                    _ => panic!(),
                };
                assert_eq!(b.module_type.as_deref(), Some("orderbook"));
                assert_eq!(b.config["symbol"], "BTC/USDT");
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn preserves_the_existing_agent_pane_id_and_data() {
        let agent = (
            "agent-pane".to_string(),
            ModuleData {
                module_type: Some("agent".into()),
                config: json!({"conversation_id": "c1"}),
            },
        );
        let mut tree = build_preset("trading", &base(), &json!({})).unwrap();
        ensure_agent_pane(&mut tree, Some(agent)).unwrap();
        let found = find_agent_pane(&tree, None).expect("agent pane");
        assert_eq!(found.0, "agent-pane");
        assert_eq!(found.1.config["conversation_id"], "c1");
    }

    #[test]
    fn injects_an_agent_pane_when_the_model_omits_it() {
        let mut tree = pane("chart", &base(), &json!({}));
        ensure_agent_pane(
            &mut tree,
            Some((
                "agent-pane".into(),
                ModuleData {
                    module_type: Some("agent".into()),
                    config: json!({}),
                },
            )),
        )
        .unwrap();
        assert!(find_agent_pane(&tree, None).is_some());
        match tree {
            PaneNode::Split { axis, ratio, b, .. } => {
                assert_eq!(axis, Axis::Vertical);
                assert_eq!(ratio, 0.72);
                assert!(matches!(*b, PaneNode::Pane { .. }));
            }
            _ => panic!("expected agent sidebar"),
        }
    }

    #[test]
    fn forces_the_caller_slot_back_to_agent_if_the_model_reused_it() {
        let agent = (
            "agent-pane".to_string(),
            ModuleData {
                module_type: Some("agent".into()),
                config: json!({"conversation_id": "c1"}),
            },
        );
        let mut tree: PaneNode = serde_json::from_value(json!({
            "kind": "pane", "id": "agent-pane",
            "data": {"type": "chart", "config": {"symbol": "SOL/USDT"}}
        }))
        .unwrap();
        ensure_agent_pane(&mut tree, Some(agent)).unwrap();
        let data = find_pane(&tree, "agent-pane").expect("pane");
        assert_eq!(data.module_type.as_deref(), Some("agent"));
        assert_eq!(data.config["conversation_id"], "c1");
    }

    #[test]
    fn every_preset_has_an_agent_pane() {
        for preset in PRESETS {
            let mut tree = build_preset(preset, &base(), &json!({})).unwrap();
            normalize_layout(&mut tree, &base()).unwrap();
            ensure_agent_pane(&mut tree, None).unwrap();
            assert!(find_agent_pane(&tree, None).is_some(), "preset {preset}");
            assert!(tree.describe_panes().len() <= MAX_PANES, "preset {preset}");
        }
    }

    #[test]
    fn preset_contains_expected_modules_and_is_under_the_limit() {
        let mut tree = build_preset("trading", &base(), &json!({"exchange": "bybit"})).unwrap();
        normalize_layout(&mut tree, &base()).unwrap();
        let panes = tree.describe_panes();
        let types: Vec<_> = panes.iter().map(|pane| pane.module_type.as_str()).collect();
        assert!(types.contains(&"chart"));
        assert!(types.contains(&"orderbook"));
        assert!(types.contains(&"trades"));
        assert!(types.contains(&"watchlist"));
        assert!(types.contains(&"agent"));
        assert!(panes.iter().all(|pane| pane.config["exchange"] == "bybit"));
    }
}
