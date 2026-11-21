//! Файл: tests/bsc_dex_multi_pair_log.rs
//! Что делает: ревью решения 295, п. 1а — лог контракта многомонетного пула пропускается, только если он учтён у ВСЕХ
//! глубоких пар контракта (listen_site::stale). Данные — ЖИВАЯ запись Curve NG (tests/data/curve_ng_2026-11-14/probe.json):
//! трёхмонетный пул 0xa5e0…, его состояние (A_precise, fee, offpeg, stored_rates, coins, balances) и настоящий лог этого
//! пула (logs[0], блок 0x7f67d6c, logIndex 0xa3). Две глубокие пары контракта (Multi): одна прочитана load_one на блоке
//! at > блока лога (synced_block = at), другая — на блоке до лога. Прежняя проверка по первой паре пропустила бы лог;
//! stale даёт вторую пару на перечитку. Событие лога у пары — «перечитать» (events::apply).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_log.
//! От чего зависит: tests/multi_common (ctx), arb::venues::bsc_dex::pools::{listen_site, load_curve_ng, events}.

mod multi_common;

use alloy::rpc::types::Log;
use alloy_primitives::{Address, Bytes};
use chrono::Utc;
use serde_json::Value;

use arb::core::types::{Pool, PoolRef, VenueId};
use arb::venues::bsc_dex::pools::events::{self, Applied};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{families, listen_site, load_curve_ng};

use multi_common::{ctx, rec};

/// Байты hex-строки записи. Паника — не hex.
fn hex(v: &Value) -> Bytes {
    Bytes::from(alloy_primitives::hex::decode(&v.as_str().unwrap()[2..]).unwrap())
}

/// Запись пары (a, b) трёхмонетного пула с состоянием из записи и отметкой синхронизации `synced`.
async fn pair(p: &Value, a: Address, b: Address, synced: u64) -> Entry {
    let c = ctx(&format!("log-{synced}-{a}")).await;
    let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
    let k = &p["calls"];
    let mut got = vec![
        Ok(hex(&k["A_precise"])),
        Ok(hex(&k["fee"])),
        Ok(hex(&k["offpeg"])),
        Ok(hex(&k["stored_rates"])),
    ];
    for (coin, bal) in p["coins"]
        .as_array()
        .unwrap()
        .iter()
        .zip(p["balances"].as_array().unwrap())
    {
        got.push(Ok(hex(coin)));
        got.push(Ok(hex(bal)));
    }
    let raw = load_curve_ng::Raw { pool, n: 3, got };
    let (t0, t1) = if a < b { (a, b) } else { (b, a) };
    let f = families::make("curve-bsc", "curve_stableswap_ng").unwrap();
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Multi {
                contract: pool,
                token0: t0,
                token1: t1,
            },
            token0: t0,
            token1: t1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: load_curve_ng::state(&c, &raw, (t0, t1)).unwrap(),
            block: synced,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: a,
        usdt: b,
        key: None,
        ticks: None,
        bins: None,
        synced_block: synced,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

#[tokio::test]
async fn log_rereads_every_unapplied_pair() {
    let probe = rec("curve_ng_2026-11-14", "probe.json");
    let p = probe["pools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["coins"].as_array().unwrap().len() == 3)
        .unwrap();
    let log: Log = serde_json::from_value(probe["logs"][0].clone()).unwrap();
    assert_eq!(
        format!("{:#x}", log.address()),
        p["pool"].as_str().unwrap(),
        "лог — этого пула"
    );
    let (block, index) = (log.block_number.unwrap(), log.log_index.unwrap());
    let coins: Vec<Address> = p["coins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Address::from_slice(&hex(c)[12..32]))
        .collect();
    // Пара A прочитана load_one позже лога (пробуждение / повтор), пара B — до лога. Блоки отметок «лог + 5» и «лог − 1»
    // ЗАДАНЫ ТЕСТОМ (не из записи): из записи — только лог (блок, номер) и состояние пула; проверяется решение о пропуске.
    let a = pair(p, coins[0], coins[1], block + 5).await;
    let mut b = pair(p, coins[0], coins[2], block - 1).await;
    assert!(a.has_applied(block, index) && !b.has_applied(block, index));
    // Лог учтён у A, но не у B: перечитывается B (прежняя проверка по первой паре пропустила бы лог целиком).
    assert_eq!(
        listen_site::stale(&[a.clone(), b.clone()], block, index, false),
        vec![1]
    );
    assert_eq!(
        listen_site::stale(&[b.clone(), a.clone()], block, index, false),
        vec![0]
    );
    // Событие лога у пары B — перечитка (у Curve NG точечного применения нет).
    assert!(matches!(
        events::apply(&mut b.clone(), &log),
        Applied::Refresh(_)
    ));
    // Учтён у обеих — пропуск; реорг — все пары.
    b.synced_block = block;
    b.synced_log = Some(index);
    assert!(listen_site::stale(&[a.clone(), b.clone()], block, index, false).is_empty());
    assert_eq!(listen_site::stale(&[a, b], block, index, true), vec![0, 1]);
}
