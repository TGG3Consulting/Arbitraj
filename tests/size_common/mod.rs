//! Файл: tests/size_common/mod.rs
//! Что делает: записанные данные CAKE для проверки подбора объёма (решение 66):
//! - стакан BingX CAKE-USDT@depth20 — tests/data/bingx/ws_spot_2026-10-23.jsonl (разбор адаптера depth::parse);
//! - правила пары CAKE-USDT (stepSize, minQty, maxQty, minNotional, maxNotional) — tests/data/bingx_2026-10-24/symbols.json;
//! - сеть BEP20 CAKE у BingX (withdrawFee, withdrawMin, depositMin, withdrawPrecision) — tests/data/bingx_2026-10-24/config_getall.json;
//! - пул CAKE/USDT PancakeSwap V2 0xa39a…755b (token0 CAKE, token1 USDT — ответы token0()/token1() в
//!   arb_node_responses_2026-10-24.json) с резервами getReserves на блоке 0x7b936ae (arb_node_responses_3_2026-10-25.json);
//!   обмен считает адаптер DEX (quote/pool_quote.rs) на настоящей SQLite с dex.v2_fee_pct = 0,25 (документация
//!   PancakeSwap V2, как настройка Тиграна).
//!
//! Ставки taker аккаунта (commissionRate), цены газа, стакана BNB и σ в записях нет — полный расчёт ядра
//! (costs::evaluate) на записях не собирается; кривая (curve.rs) — из тех же функций ядра и адаптеров.
//! Кто вызывает: tests/core_size_optimum.rs, tests/core_size_limits.rs.
//! От чего зависит: arb::core, arb::venues::{bingx, bsc_dex}, arb::settings, arb::storage.
#![allow(dead_code)]

pub mod curve;
mod dex;

pub use curve::params;

use std::str::FromStr;

use alloy_primitives::{Address, address};
use arb::core::types::{Asset, CoinNetworkInfo, PriceLevel, VenueId};
use arb::gateway::events::SpotPairState;
use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::registry::Entry;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

pub const CAKE: Address = address!("0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82");
pub const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
pub const PAIR: Address = address!("0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b");

/// Десятичное из строки.
pub fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// JSON файла.
pub fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Число из поля (строка или число — как прислал BingX); нет поля — None.
fn num(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => Some(d(s)),
        Value::Number(n) => Some(d(&n.to_string())),
        _ => None,
    }
}

/// Всё записанное о CAKE.
pub struct Rec {
    pub asks: Vec<PriceLevel>,
    pub bids: Vec<PriceLevel>,
    pub pair: SpotPairState,
    pub net: CoinNetworkInfo,
    pub pool: Entry,
    pub ctx: Ctx,
}

impl Rec {
    /// Загружает записи и окружение адаптера DEX (SQLite во временной папке теста name).
    pub async fn load(name: &str) -> Rec {
        let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
        let (mut asks, mut bids) = (vec![], vec![]);
        for l in raw.lines() {
            let m = serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string();
            if let Msg::Depth {
                symbol,
                asks: a,
                bids: b,
                ..
            } = depth::parse(&m)
                && symbol == "CAKE-USDT"
            {
                (asks, bids) = (a, b);
            }
        }
        let sym = json_file("tests/data/bingx_2026-10-24/symbols.json");
        let s = sym["data"]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["symbol"] == "CAKE-USDT")
            .unwrap();
        let pair = SpotPairState {
            ticker: "CAKE".into(),
            status: s["status"].as_i64().unwrap(),
            online: true,
            api_buy: s["apiStateBuy"].as_bool().unwrap(),
            api_sell: s["apiStateSell"].as_bool().unwrap(),
            step_size: num(&s["stepSize"]).unwrap(),
            min_notional: num(&s["minNotional"]),
            max_notional: num(&s["maxNotional"]),
            min_qty: num(&s["minQty"]),
            max_qty: num(&s["maxQty"]),
        };
        let cfg = json_file("tests/data/bingx_2026-10-24/config_getall.json");
        let coin = cfg["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["coin"] == "CAKE")
            .unwrap();
        let n = coin["networkList"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["network"] == "BEP20")
            .unwrap();
        let net = CoinNetworkInfo {
            venue: VenueId("bingx".into()),
            asset: Asset {
                ticker: "CAKE".into(),
                bsc_contract: Some(CAKE),
            },
            network: "BEP20".into(),
            deposit_enabled: true,
            withdraw_enabled: true,
            withdraw_fee: num(&n["withdrawFee"]),
            withdraw_min: num(&n["withdrawMin"]),
            withdraw_max: num(&n["withdrawMax"]),
            deposit_min: num(&n["depositMin"]),
            deposit_confirmations: None,
            withdraw_precision: n["withdrawPrecision"].as_u64().map(|p| p as u32),
            updated_at: Utc::now(),
        };
        Rec {
            asks,
            bids,
            pair,
            net,
            pool: dex::pool(),
            ctx: dex::ctx(name).await,
        }
    }
}
