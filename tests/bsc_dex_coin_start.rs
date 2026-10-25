//! Файл: tests/bsc_dex_coin_start.rs
//! Что делает: монета ждёт только СВОИХ пулов (решение 61, pools/loading.rs pending — по нему check/tax_pass.rs не
//! проверяет монету, пока её пулы загружаются) и пулы, брошенные из-за незаданной настройки, снова читаются, когда её
//! задали (reload::revive) — только они и только по своей настройке.
//! Пулы — CAKE и ETH из записи GeckoTerminal 2026-10-24 (tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json):
//! пулы ETH ещё загружаются, а у CAKE свои дочитаны — CAKE проверяется сразу, не дожидаясь ETH (на живом боте
//! 2026-10-25 четыре монеты с недочитанными пулами держали проверку всех 197).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{parts, pools::{loading, reload, discover, families}}, tests/native_common
//! (окружение на настоящей SQLite). Сети нет.

mod native_common;

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::directory::gt_parse;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::reload;

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

#[tokio::test]
async fn coin_waits_only_for_its_own_pools_and_abandoned_come_back() {
    let parts = Parts::new(native_common::ctx("coin-start").await);
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let p = parts.pools(tx);
    let cake = pools("CAKE");
    let eth = pools("ETH");
    assert!(!cake.is_empty() && !eth.is_empty());
    let (cake_addr, eth_addr) = (cake[0].0.coin, eth[0].0.coin);
    p.loading.add(cake.iter().chain(&eth).map(|(f, _)| f));
    assert_eq!(p.loading.pending(&p, cake_addr), cake.len());
    assert_eq!(p.loading.pending(&p, eth_addr), eth.len());
    // Пулы CAKE дочитаны (или брошены), ETH — ещё нет: CAKE больше не ждёт, ETH ждёт своих.
    for (f, _) in &cake {
        p.loading.done(&f.reference);
    }
    assert_eq!(p.loading.pending(&p, cake_addr), 0);
    assert_eq!(p.loading.pending(&p, eth_addr), eth.len());
    // Брошенные пулы: CAKE — из-за незаданной dex.load_batch (текст — настоящая ошибка чтения настроек бота), ETH — из-за
    // узла (текст сбоя из журнала живого запуска 2026-10-25). Изменение посторонней настройки и очистка — ничего не
    // возвращают; задали dex.load_batch — возвращаются только пулы CAKE, с нуля попыток, снова «загружаются».
    let unset = p.ctx.int("dex.load_batch").unwrap_err().message;
    let node = "узел BSC (HTTP): нет связи: error sending request".to_string();
    let mut abandoned: Vec<reload::Abandoned> = cake
        .iter()
        .enumerate()
        .map(|(i, _)| (i, &unset))
        .chain(eth.iter().enumerate().map(|(i, _)| (i, &node)))
        .map(|(i, why)| reload::Abandoned {
            i,
            setting: arb::settings::read::missing_name(why).map(String::from),
        })
        .collect();
    assert!(
        abandoned[..cake.len()]
            .iter()
            .all(|a| a.setting.as_deref() == Some("dex.load_batch"))
    );
    assert!(abandoned[cake.len()..].iter().all(|a| a.setting.is_none()));
    let back = reload::revive(&p, &cake, &mut abandoned, "notify.telegram_chat_id");
    assert!(back.is_empty(), "посторонняя настройка не возвращает пулы");
    let back = reload::revive(&p, &cake, &mut abandoned, "dex.load_batch");
    assert_eq!(back.len(), cake.len());
    assert!(back.iter().all(|m| m.tries == 0));
    assert_eq!(
        abandoned.len(),
        eth.len(),
        "брошенные из-за узла остаются ждать переподключения"
    );
    assert_eq!(p.loading.pending(&p, cake_addr), cake.len());
}
