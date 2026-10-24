//! Файл: tests/core_dex_subscription.rs
//! Что делает: подписка DEX только на монеты, которыми можно торговать на бирже (core/subscribe.rs, dex_tradable):
//! нужна сеть BSC у биржи (адрес контракта) и торгуемая спот-пара (событие SpotPairs от биржи). Данные — живой запуск
//! 2026-10-24 (tests/data/live_2026-10-24/dex_subscription.json): запись журнала BingX «нет торгуемой пары для 335
//! монет», пулы из сырого журнала узла BSC (самые частые уведомления — пулы BITLIGHT и IN), а также записанный
//! справочник пар BingX (tests/data/bingx/symbols_spot_2026-10-23.json: CAKE, TWT — status 1).
//! Адрес токена каждого пула подтверждается CREATE2 PancakeSwap V3 (адрес пула = f(деплоер, токены, комиссия)).
//! Ответ BingX о сетях монет (wallets/capital/config) не записан: флаги ввода/вывода и комиссии в тесте не
//! участвуют — dex_tradable их не смотрит.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (registry, market_state, subscribe), arb::gateway, arb::venues::bingx::symbols. Моков нет.

use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::{Address, B256, keccak256};
use chrono::Utc;
use serde_json::Value;

use arb::core::market_state::MarketState;
use arb::core::registry::{self, CoinEntry, Inputs};
use arb::core::subscribe::dex_tradable;
use arb::core::types::{Asset, CoinNetworkInfo, Pool, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::events::MarketEvent;
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::symbols;

fn fixture() -> Value {
    let raw = std::fs::read_to_string("tests/data/live_2026-10-24/dex_subscription.json").unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn addr(v: &Value) -> Address {
    Address::from_str(v.as_str().unwrap()).unwrap()
}

fn bingx() -> VenueId {
    VenueId("bingx".into())
}

/// Адрес пула PancakeSwap V3 по CREATE2: соль — keccak256(abi.encode(token0, token1, fee)).
fn pcs_v3_pool(fx: &Value, a: Address, b: Address, fee: u32) -> Address {
    let (t0, t1) = if a < b { (a, b) } else { (b, a) };
    let mut enc = [0u8; 96];
    enc[12..32].copy_from_slice(t0.as_slice());
    enc[44..64].copy_from_slice(t1.as_slice());
    enc[92..96].copy_from_slice(&fee.to_be_bytes());
    let p = &fx["pancakeswap_v3"];
    let hash = B256::from_str(p["pool_init_code_hash"].as_str().unwrap()).unwrap();
    addr(&p["deployer"]).create2(keccak256(enc), hash)
}

/// Монета BingX с сетью BSC (тикер и адрес — из живых данных). Флаги сети не записаны и в проверке не участвуют.
fn bsc_net(ticker: &str, token: Address) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: bingx(),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: Some(token),
        },
        network: "BEP20".into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        updated_at: Utc::now(),
    }
}

/// Справочник ядра по живым монетам фикстуры (как его строит core/rebuild.rs).
fn coins(fx: &Value, market: &MarketState, kinds: &[(VenueId, VenueKind)]) -> Vec<CoinEntry> {
    let nets: Vec<CoinNetworkInfo> = fx["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| bsc_net(p["ticker"].as_str().unwrap(), addr(&p["token"])))
        .collect();
    let networks = HashMap::from([(bingx(), nets)]);
    let quote = HashMap::from([(bingx(), "USDT".to_string())]);
    let bsc_network = HashMap::from([(bingx(), "BEP20".to_string())]);
    // Свежесть справочника пар (pairs_fresh.rs) на подписку DEX не влияет — не проверяется.
    let pairs_fresh = HashMap::new();
    let intermediate = HashMap::new();
    let pool_ok = |_: &Pool| Ok(());
    registry::build(&Inputs {
        venues: kinds,
        networks: &networks,
        usdt: Some(addr(&fx["usdt"])),
        quote: &quote,
        bsc_network: &bsc_network,
        pairs_fresh: &pairs_fresh,
        market,
        pool_ok: &pool_ok,
        intermediate: &intermediate,
    })
}

/// Тикеры, которые подписка DEX возьмёт при этом состоянии рынка.
fn dex_list(fx: &Value, m: &MarketState, kinds: &[(VenueId, VenueKind)]) -> Vec<String> {
    let mut out: Vec<String> = coins(fx, m, kinds)
        .iter()
        .filter(|c| dex_tradable(c, kinds, m))
        .map(|c| c.asset.ticker.clone())
        .collect();
    out.sort();
    out
}

/// SpotPairs так, как его шлёт адаптер BingX (venues/bingx/pairs.rs): монеты подписки по справочнику пар.
fn spot_pairs(fx: &Value) -> MarketEvent {
    let raw = std::fs::read_to_string("tests/data/bingx/symbols_spot_2026-10-23.json").unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let syms = symbols::Symbols::default();
    // Время получения — время стакана той же записи 2026-10-23 (tests/data/bingx/README.md); подписке DEX оно не важно.
    let at = chrono::DateTime::from_timestamp_millis(1792770730466).unwrap();
    syms.load(symbols::parse_list(&v["data"], "USDT").unwrap(), at);
    let assets: Vec<Asset> = fx["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Asset {
            ticker: p["ticker"].as_str().unwrap().to_string(),
            bsc_contract: Some(addr(&p["token"])),
        })
        .collect();
    Snapshot::build(&syms, &assets).event(bingx(), at)
}

#[test]
fn live_pools_are_these_tokens_with_usdt() {
    let fx = fixture();
    let usdt = addr(&fx["usdt"]);
    for p in fx["pools"].as_array().unwrap() {
        let fee = p["fee"].as_u64().unwrap() as u32;
        assert_eq!(
            pcs_v3_pool(&fx, addr(&p["token"]), usdt, fee),
            addr(&p["pool"]),
            "{}",
            p["ticker"]
        );
    }
    // У BITLIGHT и IN пары на BingX нет — запись журнала того же запуска.
    let msg = fx["journal_bingx_no_pair"]["message"].as_str().unwrap();
    let list: Vec<&str> = msg.split_once(": ").unwrap().1.split(", ").collect();
    assert!(list.contains(&"BITLIGHT") && list.contains(&"IN"));
    assert!(!list.contains(&"CAKE") && !list.contains(&"TWT"));
}

#[test]
fn coin_without_bingx_pair_is_not_in_dex_subscription() {
    let fx = fixture();
    let kinds = vec![
        (bingx(), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    let mut m = MarketState::default();
    // Запуск: биржа ещё не сообщила пары — DEX не подписывается ни на что (а не на всё подряд).
    assert!(dex_list(&fx, &m, &kinds).is_empty());
    m.apply(spot_pairs(&fx));
    assert_eq!(dex_list(&fx, &m, &kinds), vec!["CAKE", "TWT"]);
    // Стакан объявлен недостоверным (разрыв связи биржи) — список DEX тот же: подписка не дёргается.
    m.apply(MarketEvent::Invalidated {
        venue: bingx(),
        ticker: "CAKE".into(),
    });
    assert_eq!(dex_list(&fx, &m, &kinds), vec!["CAKE", "TWT"]);
    // Биржа выключена — её подписка пустая, пар нет: DEX слушать незачем.
    m.apply(MarketEvent::SpotPairs {
        venue: bingx(),
        tickers: vec![],
        pairs: vec![],
        loaded_at: Utc::now(),
    });
    assert!(dex_list(&fx, &m, &kinds).is_empty());
}
