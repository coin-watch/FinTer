//! Shared candle-type mapping for chart module config, menus and persistence.
//!
//! Chart panes store their render style as a string in the pane module config
//! (`type`). The renderer, the title-bar Type menu and layout persistence all
//! need to agree on that mapping, so it lives here instead of in each caller.

use gpui_component::chart::CandleType;

/// Config value + display label, in menu order.
pub const CANDLE_TYPES: [(&str, &str); 8] = [
    ("candle_solid", "Candle Solid"),
    ("candle_stroke", "Candle Stroke"),
    ("candle_up_stroke", "Candle Up Stroke"),
    ("candle_down_stroke", "Candle Down Stroke"),
    ("heikin_ashi", "Heikin-Ashi"),
    ("ohlc", "OHLC"),
    ("area", "Area"),
    ("footprint", "Footprint"),
];

/// Title-bar Type menu grouping. The grouping is intentionally different from
/// the storage order: Heikin-Ashi is a candle style, while the line/area group
/// holds the hollow/stroke variants and the area chart.
pub const CANDLE_TYPE_GROUPS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Candles",
        &[("candle_solid", "Solid"), ("heikin_ashi", "Heikin-Ashi")],
    ),
    (
        "Line & area",
        &[
            ("candle_stroke", "Stroke"),
            ("candle_up_stroke", "Up Stroke"),
            ("candle_down_stroke", "Down Stroke"),
            ("ohlc", "OHLC"),
            ("area", "Area"),
        ],
    ),
    ("Order flow", &[("footprint", "Footprint")]),
];

/// Parse the persisted pane-config value. Unknown values fall back to solid
/// candles, matching the old renderer behaviour.
pub fn candle_type_from_config(value: &str) -> CandleType {
    match value {
        "candle_stroke" => CandleType::Stroke,
        "candle_up_stroke" => CandleType::UpStroke,
        "candle_down_stroke" => CandleType::DownStroke,
        "ohlc" => CandleType::Ohlc,
        "area" => CandleType::Area,
        "heikin_ashi" => CandleType::HeikinAshi,
        "footprint" => CandleType::Footprint,
        "candle_solid" => CandleType::Solid,
        _ => CandleType::Solid,
    }
}

/// Serialize a candle type to the persisted pane-config value.
pub fn candle_type_to_config(candle_type: CandleType) -> &'static str {
    match candle_type {
        CandleType::Solid => "candle_solid",
        CandleType::Stroke => "candle_stroke",
        CandleType::UpStroke => "candle_up_stroke",
        CandleType::DownStroke => "candle_down_stroke",
        CandleType::HeikinAshi => "heikin_ashi",
        CandleType::Ohlc => "ohlc",
        CandleType::Area => "area",
        CandleType::Footprint => "footprint",
    }
}

/// Display label for a persisted pane-config value.
pub fn candle_type_label(value: &str) -> &'static str {
    CANDLE_TYPES
        .iter()
        .find_map(|(config, label)| (*config == value).then_some(*label))
        .unwrap_or("Candle Solid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_config_value_round_trips() {
        for (config, label) in CANDLE_TYPES {
            let candle_type = candle_type_from_config(config);
            assert_eq!(candle_type_to_config(candle_type), config);
            assert_eq!(candle_type_label(config), label);
        }
    }

    #[test]
    fn footprint_style_persists() {
        assert_eq!(candle_type_from_config("footprint"), CandleType::Footprint);
        assert_eq!(candle_type_to_config(CandleType::Footprint), "footprint");
        assert_eq!(candle_type_label("footprint"), "Footprint");
    }

    #[test]
    fn unknown_config_falls_back_to_solid() {
        assert_eq!(candle_type_from_config("not-a-style"), CandleType::Solid);
        assert_eq!(candle_type_label("not-a-style"), "Candle Solid");
    }

    #[test]
    fn type_menu_groups_cover_every_config_exactly_once() {
        let mut seen = Vec::new();
        for (_, group) in CANDLE_TYPE_GROUPS {
            for (config, label) in group {
                assert!(CANDLE_TYPES.iter().any(|(value, _)| value == config));
                assert!(!label.is_empty());
                assert!(!seen.contains(config), "{config} appears in two groups");
                seen.push(*config);
            }
        }
        assert_eq!(seen.len(), CANDLE_TYPES.len());
        for (config, _) in CANDLE_TYPES {
            assert!(
                seen.contains(&config),
                "{config} is missing from the menu groups"
            );
        }
    }

    #[test]
    fn menu_grouping_is_the_requested_split() {
        assert_eq!(
            CANDLE_TYPE_GROUPS[0].1,
            &[("candle_solid", "Solid"), ("heikin_ashi", "Heikin-Ashi")]
        );
        assert_eq!(
            CANDLE_TYPE_GROUPS[1].1,
            &[
                ("candle_stroke", "Stroke"),
                ("candle_up_stroke", "Up Stroke"),
                ("candle_down_stroke", "Down Stroke"),
                ("ohlc", "OHLC"),
                ("area", "Area"),
            ]
        );
    }
}
