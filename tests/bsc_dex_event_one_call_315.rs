//! Файл: tests/bsc_dex_event_one_call_315.rs
//! Что делает: решение 315 — разбор ответа «одного обращения» к узлу для пула V3 (pools/load_v3_once.rs assemble) на
//! РЕАЛЬНЫХ ответах узла BSC: tests/data/squadswap_v3_2026-11-15/probe.json — три пула SquadSwap V3 (ABI PancakeSwap
//! V3), все eth_call на одном блоке 133618816: slot0, liquidity, fee(), TickLens.getPopulatedTicksInWord по словам ±10
//! от слова текущего тика.
//! Записи ответа Multicall3.aggregate3 нет: ответы вызовов подаются в assemble списком «байты каждого вызова» — ровно
//! то, что node/multicall.rs decode отдаёт из returnData; сами байты — записанные ответы eth_call тех же функций на том
//! же блоке (их обёртку aggregate3 этот тест не проверяет — запись снять, docs/TASKS.md, решение 315).
//! Проверяется: итог (состояние V3, тики, диапазон слов, fee()) бит-в-бит равен последовательному чтению на том же
//! блоке (slot0 → liquidity → слова настоящего диапазона → fee(), разбор по ABI в тесте); слова сверх настоящего
//! диапазона отбрасываются, их откат итог не трогает; откат нужного слова — ошибка; настоящий диапазон шире
//! прочитанного — Words (второе обращение) с той же головой.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{load_v3_once, load_batch, fee_fresh, families, abi_v2v3, abi_check,
//! ticks}.
//! Настройки: нет (диапазон «настоящих» слов задаёт тест: ±2 слова — dex.tick_words сервера 15.11, решение 296).

use std::collections::BTreeMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::{PoolRef, PoolState};
use arb::venues::bsc_dex::pools::abi_check::IPoolImmutables;
use arb::venues::bsc_dex::pools::abi_v2v3::{IPancakeV3Pool, ITickLens, IV3Pool};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::fee_fresh::Extras;
use arb::venues::bsc_dex::pools::load_batch::{Miss, Want};
use arb::venues::bsc_dex::pools::load_v3_once::{Got, assemble};
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/squadswap_v3_2026-11-15/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn bytes(v: &Value) -> Bytes {
    let s = v.as_str().unwrap().trim_start_matches("0x");
    Bytes::from(alloy_primitives::hex::decode(s).unwrap())
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Записанные слова пула: номер слова → ответ TickLens.
fn recorded_words(p: &Value) -> BTreeMap<i16, Bytes> {
    p["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["word"].as_i64().unwrap() as i16, bytes(&w["raw"])))
        .collect()
}

/// Последовательное чтение на том же блоке, разобранное в тесте: состояние, тики слов `range`, fee().
fn sequential(
    c: &Value,
    words: &BTreeMap<i16, Bytes>,
    range: (i16, i16),
) -> (PoolState, BTreeMap<i32, TickData>, u32) {
    let s = IPancakeV3Pool::slot0Call::abi_decode_returns(&bytes(&c["slot0"])).unwrap();
    let liquidity = IV3Pool::liquidityCall::abi_decode_returns(&bytes(&c["liquidity"])).unwrap();
    let state = PoolState::V3 {
        sqrt_price_x96: U256::from(s.sqrtPriceX96),
        liquidity,
        tick: s.tick.as_i32(),
    };
    let mut ticks = BTreeMap::new();
    for w in range.0..=range.1 {
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(&words[&w]).unwrap() {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    let fee: u32 = IPoolImmutables::feeCall::abi_decode_returns(&bytes(&c["fee"]))
        .unwrap()
        .to();
    (state, ticks, fee)
}

/// Ответ одного обращения: голова (slot0, liquidity, fee() — порядок load_batch_v2v3 head при fee() 281) и слова `pred`.
fn answer(c: &Value, words: &BTreeMap<i16, Bytes>, pred: (i16, i16)) -> Vec<Result<Bytes, String>> {
    let mut a = vec![
        Ok(bytes(&c["slot0"])),
        Ok(bytes(&c["liquidity"])),
        Ok(bytes(&c["fee"])),
    ];
    a.extend((pred.0..=pred.1).map(|w| Ok(words[&w].clone())));
    a
}

#[test]
fn one_call_equals_sequential_read_on_recorded_block() {
    let d = probe();
    let f = families::make("squadswap-v3-bsc", "v3_fork_pancake").unwrap();
    let x = Extras {
        swap_fee: false,
        fresh: true,
    };
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let words = recorded_words(p);
        let spacing = U256::from_be_slice(&bytes(&c["tickSpacing"])).to::<u64>() as i32;
        let r = PoolRef::Contract(addr(&p["pool"]));
        let w = Want {
            family: f,
            reference: &r,
            spacing: Some(spacing),
            step: None,
            usdt: addr(&p["token1"]),
            token0: addr(&p["token0"]),
            token1: addr(&p["token1"]),
        };
        let pred = (*words.keys().next().unwrap(), *words.keys().last().unwrap());
        // Настоящий диапазон — ±2 слова от слова тика slot0 (внутри записанных ±10).
        let near = |t: i32| -> Result<(i16, i16), Miss> {
            let c = i32::from(TickMap::word_of(t, spacing));
            Ok(((c - 2) as i16, (c + 2) as i16))
        };
        let ans = answer(c, &words, pred);
        let Got::Done(l) = assemble(&w, x, 3, pred, &ans, &near).unwrap() else {
            panic!("диапазон внутри прочитанного — одно обращение");
        };
        let s0 = IPancakeV3Pool::slot0Call::abi_decode_returns(&bytes(&c["slot0"])).unwrap();
        let range = near(s0.tick.as_i32()).unwrap();
        let (state, ticks, fee) = sequential(c, &words, range);
        assert_eq!(l.state, state);
        assert_eq!(l.fee_pips, Some(fee));
        let t = l.ticks.expect("карта тиков");
        assert_eq!((t.spacing, t.words), (spacing, range));
        assert_eq!(*t.ticks, ticks);
        assert!(l.bins.is_none());

        // Откат лишнего слова итог не трогает; откат нужного — ошибка.
        let mut bad = ans.clone();
        bad[3] = Err("вызов откатился: 0x".into());
        assert!(matches!(
            assemble(&w, x, 3, pred, &bad, &near),
            Ok(Got::Done(_))
        ));
        let mut bad = ans.clone();
        bad[3 + (i32::from(range.0) - i32::from(pred.0)) as usize] =
            Err("вызов откатился: 0x".into());
        assert!(assemble(&w, x, 3, pred, &bad, &near).is_err());
        // Откат головы — ошибка.
        let mut bad = ans.clone();
        bad[1] = Err("вызов откатился: 0x".into());
        assert!(assemble(&w, x, 3, pred, &bad, &near).is_err());

        // Настоящий диапазон шире прочитанного — второе обращение за его словами, голова та же.
        let wide = |_: i32| -> Result<(i16, i16), Miss> { Ok((pred.0 - 1, pred.1)) };
        match assemble(&w, x, 3, pred, &ans, &wide).unwrap() {
            Got::Words {
                state: s,
                fee: fe,
                range: rg,
            } => {
                assert_eq!(s, state);
                assert_eq!(fe, Some(fee));
                assert_eq!(rg, (pred.0 - 1, pred.1));
            }
            Got::Done(_) => panic!("диапазон шире прочитанного — нужны другие слова"),
        }
    }
}
