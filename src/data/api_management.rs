//! Local-only exchange API credentials.
//!
//! Credentials are encrypted with AES-256-GCM before being written to
//! `~/.config/finter/api_providers.json` (mode 0600). The encryption key lives
//! in the OS keyring and is only loaded into process memory while FinTer is
//! running, so the on-disk file is ciphertext at rest. Nothing is synced to the
//! remote server or included in cloud backups.
use anyhow::{Context, Result};
use gpui::SharedString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use zeroize::Zeroizing;

use crate::data::credential_crypto;

pub const CCXT_EXCHANGES: &[&str] = &[
    "alpaca", "apex", "aster", "backpack", "bequant", "bigone", "binance",
    "binancecoinm", "binanceus", "binanceusdm", "bingx", "bit2c", "bitbank",
    "bitbns", "bitfinex", "bitflyer", "bitget", "bithumb", "bitopro", "bitrue",
    "bitso", "bitstamp", "bitteam", "bittrade", "bitvavo", "blockchaincom",
    "blofin", "btcbox", "btcmarkets", "btcturk", "btse", "bullish", "bybit",
    "bybiteu", "bybitid", "bydfi", "cex", "coinbase", "coinbaseexchange",
    "coinbaseinternational", "coincheck", "coinmate", "coinone", "coinsph",
    "coinspot", "cryptocom", "cryptomus", "deepcoin", "delta", "deribit",
    "derive", "digifinex", "dydx", "extended", "fmfwio", "foxbit", "gate",
    "gateeu", "gemini", "grvt", "hashkey", "hibachi", "hitbtc", "hollaex",
    "htx", "hyperliquid", "independentreserve", "indodax", "kraken",
    "krakenfutures", "kucoin", "kucoinfutures", "latoken", "lbank", "lighter",
    "luno", "mercado", "mexc", "mod", "modetrade", "mudrex", "myokx", "nado",
    "ndax", "okx", "okxus", "onetrading", "p2b", "pacifica", "paradex",
    "paymium", "phemex", "poloniex", "polymarket", "revolutx", "tokocrypto",
    "toobit", "upbit", "weex", "whitebit", "woo", "woofipro", "xt", "zaif",
    "zebpay",
];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExchangeCredential {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub exchange: String,
    pub api_key: String,
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(default)]
    pub is_testnet: bool,
    #[serde(default)]
    pub created_at: Option<u64>,
}

// For compatibility with any previously saved ApiProvider
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiProvider {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub api_key: String,
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: String,
}

fn credentials_file_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("finter/api_providers.json"))
}

/// Fill in stable ids and labels for credentials that predate those fields.
fn normalize(list: &mut [ExchangeCredential]) {
    for (idx, item) in list.iter_mut().enumerate() {
        if item.id.trim().is_empty() {
            item.id = format!("{}-{}", item.exchange.to_lowercase(), idx + 1);
        }
        if item.label.trim().is_empty() {
            item.label = format!("{} Account", item.exchange.to_uppercase());
        }
    }
}

/// Parse the pre-encryption on-disk formats.
///
/// Returns `None` when the bytes are neither the current `ExchangeCredential`
/// list nor the legacy `ApiProvider` list.
fn parse_plaintext(bytes: &[u8]) -> Option<Vec<ExchangeCredential>> {
    // New ExchangeCredential list.
    if let Ok(mut list) = serde_json::from_slice::<Vec<ExchangeCredential>>(bytes) {
        if !list.is_empty() && !list[0].exchange.is_empty() {
            normalize(&mut list);
            return Some(list);
        }
    }

    // Legacy ApiProvider list.
    if let Ok(list) = serde_json::from_slice::<Vec<ApiProvider>>(bytes) {
        let mut mapped: Vec<ExchangeCredential> = list
            .into_iter()
            .enumerate()
            .map(|(idx, p)| {
                let ex = if !p.base_url.is_empty() {
                    p.base_url
                } else {
                    p.name.to_lowercase()
                };
                ExchangeCredential {
                    id: format!("{}-{}", ex.to_lowercase(), idx + 1),
                    label: if !p.name.is_empty() {
                        p.name
                    } else {
                        format!("{} Key", ex.to_uppercase())
                    },
                    exchange: ex,
                    api_key: p.api_key,
                    api_secret: p.api_secret,
                    passphrase: p.passphrase,
                    is_testnet: false,
                    created_at: None,
                }
            })
            .collect();
        normalize(&mut mapped);
        return Some(mapped);
    }

    None
}

pub fn load() -> Vec<ExchangeCredential> {
    let Some(path) = credentials_file_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };

    if credential_crypto::is_encrypted(&bytes) {
        return match credential_crypto::decrypt(&bytes) {
            Ok(plaintext) => match serde_json::from_slice::<Vec<ExchangeCredential>>(&plaintext) {
                Ok(mut list) => {
                    normalize(&mut list);
                    list
                }
                Err(e) => {
                    tracing::error!("decrypted API credentials are malformed: {e}");
                    Vec::new()
                }
            },
            Err(e) => {
                tracing::error!("failed to decrypt API credentials: {e:#}");
                Vec::new()
            }
        };
    }

    // Legacy plaintext file: read it, then re-save it encrypted in place.
    let Some(list) = parse_plaintext(&bytes) else {
        tracing::error!(
            "API credential file {} is neither encrypted nor a recognised legacy format",
            path.display()
        );
        return Vec::new();
    };

    tracing::warn!("migrating plaintext API credentials to encrypted storage");
    if let Err(e) = save(&list) {
        tracing::error!("failed to encrypt migrated API credentials: {e:#}");
    }
    list
}

pub fn save(items: &[ExchangeCredential]) -> Result<()> {
    use std::io::Write;

    let path = credentials_file_path().context("Local configuration directory unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Keep the containing directory owner-only as well.
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let plaintext = Zeroizing::new(serde_json::to_vec(items)?);
    let encrypted = credential_crypto::encrypt(&plaintext[..])?;

    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(&encrypted)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Exchange ids that share the same API credentials.
///
/// CCXT exposes some venues as several market-specific ids even though the
/// venue issues one API key for all of them. Binance spot/USDⓈ-M/COIN-M and
/// KuCoin spot/futures are the common cases. Aliases for rebranded venues are
/// included so credentials saved under an older id keep working.
///
/// Note: Kraken Futures, Coinbase Exchange, Binance US, OKX US, and the
/// regional Bybit/Gate entities issue separate keys and are intentionally not
/// merged here.
pub fn credential_family(exchange: &str) -> Vec<String> {
    let ex = exchange.trim().to_ascii_lowercase();
    let members: &[&str] = match ex.as_str() {
        "binance" | "binanceusdm" | "binancecoinm" => &["binance", "binanceusdm", "binancecoinm"],
        "kucoin" | "kucoinfutures" => &["kucoin", "kucoinfutures"],
        "htx" | "huobi" => &["htx", "huobi"],
        "gate" | "gateio" => &["gate", "gateio"],
        "okx" | "okex" => &["okx", "okex"],
        "mexc" | "mxc" => &["mexc", "mxc"],
        _ => return vec![ex],
    };
    members.iter().map(|id| (*id).to_string()).collect()
}

pub fn get_credential(id_or_exchange: &str) -> Option<ExchangeCredential> {
    let target = id_or_exchange.trim().to_lowercase();
    let all = load();
    if let Some(found) = all
        .iter()
        .find(|c| c.id.to_lowercase() == target || c.exchange.to_lowercase() == target)
    {
        return Some(found.clone());
    }
    let family = credential_family(&target);
    if family.len() <= 1 {
        return None;
    }
    all.into_iter()
        .find(|c| family.iter().any(|id| c.exchange.to_lowercase() == *id))
}

pub fn get_all_credentials() -> Vec<ExchangeCredential> {
    load()
}

pub fn get_credentials_for_exchange(exchange: &str) -> Vec<ExchangeCredential> {
    let ex_lower = exchange.trim().to_lowercase();
    let all = load();
    let exact: Vec<ExchangeCredential> = all
        .iter()
        .filter(|c| c.exchange.to_lowercase() == ex_lower)
        .cloned()
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let family = credential_family(&ex_lower);
    if family.len() <= 1 {
        return Vec::new();
    }
    all.into_iter()
        .filter(|c| family.iter().any(|id| c.exchange.to_lowercase() == *id))
        .collect()
}

pub fn has_credential(exchange: &str) -> bool {
    get_credential(exchange).map(|c| !c.api_key.trim().is_empty()).unwrap_or(false)
}

pub fn save_exchange_credential(mut cred: ExchangeCredential) -> Result<()> {
    let mut list = load();
    if cred.id.trim().is_empty() {
        cred.id = uuid::Uuid::new_v4().to_string();
    }
    if cred.label.trim().is_empty() {
        cred.label = format!("{} Account", cred.exchange.to_uppercase());
    }
    if cred.created_at.is_none() {
        cred.created_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
    }
    if let Some(existing) = list.iter_mut().find(|c| c.id == cred.id) {
        *existing = cred;
    } else {
        list.push(cred);
    }
    save(&list)
}

pub fn delete_exchange_credential(id: &str) -> Result<()> {
    let mut list = load();
    list.retain(|c| c.id != id);
    save(&list)
}

/// Lists exchanges appropriate to the current authentication state.
/// Logged-in users see precisely the CoinWatch `/api/v1/exchanges` catalogue;
/// logged-out users see the exchanges shipped by CCXT for direct API access.
pub fn supported_api_exchanges() -> Vec<(SharedString, SharedString)> {
    let logged_in = crate::data::session_store::load_session()
        .ok()
        .flatten()
        .is_some();

    let exchanges: Vec<String> = if logged_in {
        let registry = crate::data::gateway_registry::GatewayRegistry::global();
        registry.api_loaded_exchanges().unwrap_or_else(|| registry.exchanges())
    } else {
        CCXT_EXCHANGES.iter().map(|ex| (*ex).to_string()).collect()
    };

    let mut set = BTreeSet::new();
    for exchange in exchanges {
        let clean = exchange.trim().to_lowercase();
        if !clean.is_empty() {
            set.insert(clean);
        }
    }
    set.into_iter()
        // Only offer venues that this build can actually use for live API calls.
        .filter(|ex| crate::data::exchange_balance::is_exchange_available(ex))
        .map(|ex| (SharedString::from(ex.clone()), SharedString::from(ex.to_uppercase())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credential_families_share_keys() {
        assert!(credential_family("binanceusdm").contains(&"binance".to_string()));
        assert!(credential_family("binancecoinm").contains(&"binance".to_string()));
        assert!(credential_family("kucoinfutures").contains(&"kucoin".to_string()));
        assert!(credential_family("huobi").contains(&"htx".to_string()));

        // Separate venues/entities must not share credentials.
        assert_eq!(
            credential_family("krakenfutures"),
            vec!["krakenfutures".to_string()]
        );
        assert_eq!(
            credential_family("binanceus"),
            vec!["binanceus".to_string()]
        );
        assert_eq!(credential_family("bybiteu"), vec!["bybiteu".to_string()]);
        assert_eq!(
            credential_family("coinbaseexchange"),
            vec!["coinbaseexchange".to_string()]
        );
    }

    #[test]
    fn test_exchange_credential_serialization_and_defaults() {
        let json_input = r#"[
            {
                "exchange": "binance",
                "api_key": "key1",
                "api_secret": "sec1"
            },
            {
                "id": "custom-bybit-1",
                "label": "Scalp Bot",
                "exchange": "bybit",
                "api_key": "key2",
                "api_secret": "sec2",
                "passphrase": "pass",
                "is_testnet": true
            }
        ]"#;

        let creds: Vec<ExchangeCredential> = serde_json::from_str(json_input).unwrap();
        assert_eq!(creds.len(), 2);
        assert_eq!(creds[0].exchange, "binance");
        assert_eq!(creds[0].id, "");
        assert_eq!(creds[0].is_testnet, false);

        assert_eq!(creds[1].id, "custom-bybit-1");
        assert_eq!(creds[1].label, "Scalp Bot");
        assert_eq!(creds[1].passphrase, "pass");
        assert_eq!(creds[1].is_testnet, true);
    }

    #[test]
    fn test_legacy_plaintext_detection_and_normalization() {
        let json = br#"[{"exchange":"binance","api_key":"k","api_secret":"s"}]"#;
        let list = parse_plaintext(json).expect("legacy plaintext list");
        assert_eq!(list[0].id, "binance-1");
        assert_eq!(list[0].label, "BINANCE Account");
    }

    #[test]
    fn test_legacy_api_provider_migration() {
        let json = br#"[{"name":"My Binance","kind":"cex","base_url":"binance","api_key":"k","api_secret":"s"}]"#;
        let list = parse_plaintext(json).expect("legacy provider list");
        assert_eq!(list[0].exchange, "binance");
        assert_eq!(list[0].label, "My Binance");
    }

    #[test]
    fn test_encrypted_envelope_is_not_plaintext() {
        let envelope =
            br#"{"version":1,"alg":"AES-256-GCM","nonce":"AAAA","ciphertext":"AAAA"}"#;
        assert!(credential_crypto::is_encrypted(envelope));
        assert!(parse_plaintext(envelope).is_none());
        assert!(!credential_crypto::is_encrypted(br#"[{"exchange":"binance"}]"#));
    }

    #[test]
    fn test_multi_keys_for_same_exchange() {
        let cred1 = ExchangeCredential {
            id: "binance-spot".into(),
            label: "Binance Spot".into(),
            exchange: "binance".into(),
            api_key: "k1".into(),
            api_secret: "s1".into(),
            passphrase: "".into(),
            is_testnet: false,
            created_at: Some(100),
        };
        let cred2 = ExchangeCredential {
            id: "binance-futures".into(),
            label: "Binance Futures Subaccount".into(),
            exchange: "binance".into(),
            api_key: "k2".into(),
            api_secret: "s2".into(),
            passphrase: "".into(),
            is_testnet: false,
            created_at: Some(200),
        };

        let list = vec![cred1, cred2];
        let serialized = serde_json::to_string(&list).unwrap();
        let deserialized: Vec<ExchangeCredential> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.len(), 2);
        assert_eq!(deserialized[0].exchange, "binance");
        assert_eq!(deserialized[1].exchange, "binance");
        assert_ne!(deserialized[0].id, deserialized[1].id);
    }
}
