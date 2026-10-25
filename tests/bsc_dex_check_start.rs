//! Файл: tests/bsc_dex_check_start.rs
//! Что делает: проверки пулов на старте подписки (решение 65, check/fresh.rs). Случай живого бота (main 43bda1f): первый
//! проход сверки с квотером и крючков шёл, пока пулы ещё не прочитаны («прошли 0, исключены 0»), а дочитанные позже
//! пулы ждали dex.quoter_check_s (900 с) — 15 минут без цен. Теперь пул проверяется, как только он стал достоверным и
//! циклом ещё не проверен; сводка первого прохода — когда ни один пул не читается (застрявшие из-за незаданной
//! настройки не держат) и все прочитанные проверены.
//! Пулы — CAKE и ETH из записи GeckoTerminal 2026-10-24 (tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{parts, check::fresh, pools::{loading, registry, discover, families}},
//! tests/native_common (окружение на настоящей SQLite, вид записи реестра). Сети нет.

mod native_common;

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::check::fresh::Fresh;
use arb::venues::bsc_dex::directory::gt_parse;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;

/// Пулы V2/V3 монеты из записи индексатора (адрес пула — контракт; вид — по DEX записи).
fn pools(coin: &str) -> Vec<(Found, String)> {
    let text =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    gt_parse::pools(rec[format!("gt_pools_{coin}_p1")]["body"].as_str().unwrap())
        .unwrap()
        .into_iter()
        .filter(|p| p.pool.len() == 42)
        .filter_map(|p| {
            let kind = match p.dex_id.as_str() {
                "pancakeswap_v2" => "pancakeswap_v2",
                "pancakeswap-v3-bsc" => "pancakeswap_v3",
                "uniswap-bsc" => "uniswap_v3",
                _ => return None,
            };
            let family = families::make(&p.dex_id, kind)?;
            let token = if p.base_symbol.eq_ignore_ascii_case(coin) {
                p.base
            } else {
                p.quote
            };
            let f = Found {
                family,
                reference: PoolRef::Contract(p.pool.parse::<Address>().ok()?),
                token0: p.base.min(p.quote),
                token1: p.base.max(p.quote),
                coin: token,
                fee_pips: None,
                tick_spacing: None,
                bin_step: None,
                key: None,
                hooks: None,
            };
            Some((f, coin.to_string()))
        })
        .collect()
}

/// Пул прочитан: запись реестра с его адресом и монетой (вид записи — native_common::entry).
fn read(f: &Found, ticker: &str, valid: bool) -> Entry {
    let mut e = native_common::entry("pancakeswap_v2", None);
    e.pool.reference = f.reference.clone();
    e.coin = f.coin;
    e.ticker = ticker.into();
    e.valid = valid;
    e
}

#[tokio::test]
async fn pool_is_checked_as_soon_as_read_and_summary_waits_for_reading_pools() {
    let parts = Parts::new(native_common::ctx("check-start").await);
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let p = parts.pools(tx);
    let (cake, eth) = (pools("CAKE"), pools("ETH"));
    assert!(!cake.is_empty() && eth.len() > 1);
    let mut fresh = Fresh::default();
    // Старт: пулы найдены, ни один не прочитан — проверять нечего, сводки нет (раньше — «прошли 0, исключены 0»).
    p.loading.add(cake.iter().chain(&eth).map(|(f, _)| f));
    assert!(fresh.due(&p, |_| false).is_empty());
    assert!(!fresh.settled(&p), "пулы читаются — сводки нет");
    // Пулы CAKE прочитаны — сразу к проверке, не через интервал полного прохода.
    for (f, t) in &cake {
        p.reg.put(read(f, t, true));
        p.loading.done(&f.reference);
    }
    let due = fresh.due(&p, |_| false);
    assert_eq!(due.len(), cake.len());
    for r in &due {
        fresh.mark(r);
    }
    assert!(
        fresh.due(&p, |_| false).is_empty(),
        "проверенный пул второй раз не берётся"
    );
    assert!(!fresh.settled(&p), "пулы ETH ещё читаются");
    // Повтор — только по условию (сверка ждала влияния крючка, а оно измерено).
    let first: PoolRef = due[0].clone();
    assert_eq!(fresh.due(&p, |r| *r == first), vec![first.clone()]);
    // Первый пул ETH прочитан, второй — недостоверен (данным верить нельзя), остальные застряли (ждут настройку).
    p.reg.put(read(&eth[0].0, "ETH", true));
    p.loading.done(&eth[0].0.reference);
    p.reg.put(read(&eth[1].0, "ETH", false));
    p.loading.done(&eth[1].0.reference);
    for (f, _) in &eth[2..] {
        p.loading.stuck(&f.reference, true);
    }
    assert_eq!(fresh.due(&p, |_| false), vec![eth[0].0.reference.clone()]);
    assert!(!fresh.settled(&p), "прочитанный пул ETH ещё не проверен");
    fresh.mark(&eth[0].0.reference);
    assert!(
        fresh.settled(&p),
        "читающихся нет, прочитанные проверены — сводка"
    );
    // Застрявший пул снова читается — сводку первого прохода держал бы снова (если она ещё не отправлена).
    if eth.len() > 2 {
        p.loading.stuck(&eth[2].0.reference, false);
        assert!(!fresh.settled(&p));
    }
}
