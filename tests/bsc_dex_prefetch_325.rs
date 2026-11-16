//! Файл: tests/bsc_dex_prefetch_325.rs
//! Что делает: решение 325 на РЕАЛЬНЫХ ответах узла BSC (tests/data/algebra_integral_2026-11-06/probe.json, блок
//! 132036268, пулы Algebra Integral thena-v3 и dexswap-bsc, обход списка тиков ticks() вверх/вниз — записан узлом):
//! 1. prefetch_integral::walk_plan по битам дерева тиков даёт ТОТ ЖЕ порядок тиков, что записанный обход списка
//!    (nextTickGlobal → поле nextTick ответа ticks() … ; prevTickGlobal → prevTick …), включая конец списка
//!    (MAX_TICK / MIN_TICK). Биты дерева строятся из тиков записанного списка: тики списка — ровно установленные биты
//!    дерева (TickTree.sol / TickStructure.sol _addOrRemoveTick, integral-core 1.2.x; abi_algebra_integral.rs).
//!    Записи самих ответов tickTable / tickTreeSecondLayer нет — снять (docs/TASKS.md, решение 325).
//! 2. Индексы второго слоя (nodes, nonempty) — по формуле TickTree.sol (SECOND_LAYER_OFFSET = 3466): весь диапазон
//!    тиков — узлы 0..=27; «лист не пуст» совпадает с наличием тиков в листе.
//! 3. Книга заранее прочитанных ответов (node/prefetch.rs): ответ globalState из записи отдаётся только на своём
//!    блоке и только внутри scope, откат в книгу не кладётся, повторно заранее не спрашивается; разбор ответа из книги
//!    равен разбору записанного ответа.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_integral_common (запись), arb::venues::bsc_dex::pools::{prefetch_integral,
//! abi_algebra_integral}, arb::venues::bsc_dex::node::{prefetch, call}.
//! Настройки: нет.

mod algebra_integral_common;

use std::collections::BTreeSet;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::tick_math::{MAX_TICK, MIN_TICK};
use arb::venues::bsc_dex::node::call;
use arb::venues::bsc_dex::node::prefetch::{self, Book};
use arb::venues::bsc_dex::pools::abi_algebra_integral::{
    IAlgebraIntegralPool as P, SECOND_LAYER_OFFSET,
};
use arb::venues::bsc_dex::pools::prefetch_integral::{leaf, nodes, nonempty, walk_plan};

use algebra_integral_common::{probe, signed};

fn hex(v: &Value) -> Bytes {
    Bytes::from(
        alloy_primitives::hex::decode(v.as_str().unwrap().trim_start_matches("0x")).unwrap(),
    )
}

fn list(p: &Value, side: &str) -> Vec<i32> {
    p[side]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tick"].as_i64().unwrap() as i32)
        .collect()
}

/// Листы дерева и второй слой из тиков списка (концы списка MIN_TICK / MAX_TICK — не биты, TickManagement.sol).
fn tree(ticks: &BTreeSet<i32>) -> (Vec<(i16, U256)>, Vec<U256>) {
    let mut leaves: Vec<(i16, U256)> = Vec::new();
    for t in ticks.iter().filter(|t| **t != MIN_TICK && **t != MAX_TICK) {
        let (w, b) = (leaf(*t), (*t & 0xFF) as usize);
        match leaves.iter_mut().find(|(l, _)| *l == w) {
            Some((_, word)) => word.set_bit(b, true),
            None => {
                let mut word = U256::ZERO;
                word.set_bit(b, true);
                leaves.push((w, word));
            }
        }
    }
    let first = i32::from(*nodes().start());
    let mut layer = vec![U256::ZERO; nodes().count()];
    for (w, _) in &leaves {
        let i = i32::from(*w) + SECOND_LAYER_OFFSET;
        layer[((i >> 8) - first) as usize].set_bit((i & 0xFF) as usize, true);
    }
    (leaves, layer)
}

#[test]
fn walk_plan_equals_recorded_list_walk() {
    let d = probe();
    for p in d["pools"].as_array().unwrap() {
        let (up, down) = (list(p, "ticks_up"), list(p, "ticks_down"));
        let all: BTreeSet<i32> = up.iter().chain(down.iter()).copied().collect();
        let (leaves, layer) = tree(&all);
        let ne = |l: i16| nonempty(&layer, l);
        let bits = |l: i16| leaves.iter().find(|(w, _)| *w == l).map(|(_, b)| *b);
        let next = signed(p["calls"]["nextTickGlobal"]["raw"].as_str().unwrap(), 0) as i32;
        let prev = signed(p["calls"]["prevTickGlobal"]["raw"].as_str().unwrap(), 0) as i32;
        let a = walk_plan(next, true, up.len() as u64, &ne, &bits);
        let b = walk_plan(prev, false, down.len() as u64, &ne, &bits);
        assert_eq!(a.ticks, up, "{} вверх", p["pool"]);
        assert_eq!(b.ticks, down, "{} вниз", p["pool"]);
        assert!(a.need.is_empty() && b.need.is_empty(), "{}", p["pool"]);
        // Лист не прочитан — план останавливается и просит его (и непустые листы дальше).
        if up.len() > 1 && up[1] != MAX_TICK {
            let unknown = leaf(up[1]);
            let hidden = |l: i16| if l == unknown { None } else { bits(l) };
            let c = walk_plan(next, true, up.len() as u64, &ne, &hidden);
            assert_eq!(c.need.first(), Some(&unknown), "{}", p["pool"]);
            assert_eq!(c.ticks, vec![up[0]], "{}", p["pool"]);
        }
    }
}

#[test]
fn second_layer_indexes_follow_tick_tree() {
    // TickTree.sol: узел второго слоя = sar(8, лист + 3466); MIN_TICK → узел 0, MAX_TICK → узел 27.
    assert_eq!(nodes(), 0..=27);
    let d = probe();
    for p in d["pools"].as_array().unwrap() {
        let all: BTreeSet<i32> = list(p, "ticks_up")
            .into_iter()
            .chain(list(p, "ticks_down"))
            .collect();
        let (leaves, layer) = tree(&all);
        for l in leaf(MIN_TICK)..=leaf(MAX_TICK) {
            assert_eq!(
                nonempty(&layer, l),
                leaves.iter().any(|(w, _)| *w == l),
                "лист {l}"
            );
        }
    }
}

#[tokio::test]
async fn book_serves_recorded_answer_only_on_its_block() {
    let d = probe();
    let p = &d["pools"][0];
    let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
    let gs = (pool, Bytes::from(P::globalStateCall {}.abi_encode()));
    let fee = (pool, Bytes::from(P::feeCall {}.abi_encode()));
    let raw = hex(&p["calls"]["globalState"]["raw"]);
    let tag = d["block_hex"].as_str().unwrap().to_string();
    let book = Book::new(&tag);
    book.fill(
        &[gs.clone(), fee.clone()],
        &[Ok(raw.clone()), Err("откат".into())],
    );
    // Вне scope книги нет — вызов пошёл бы к узлу.
    assert_eq!(prefetch::hit(gs.0, &gs.1, &tag), None);
    let (a, other, failed) = prefetch::scope(book.clone(), async {
        (
            prefetch::hit(gs.0, &gs.1, &tag),
            prefetch::hit(gs.0, &gs.1, "latest"),
            prefetch::hit(fee.0, &fee.1, &tag),
        )
    })
    .await;
    assert_eq!(a, Some(raw.clone()));
    assert_eq!(other, None);
    assert_eq!(failed, None);
    assert_eq!(book.counts(), (1, 1));
    assert_eq!(book.used(), vec![gs.clone(), fee.clone()]);
    assert!(book.fresh(vec![gs.clone(), fee]).is_empty());
    let x = call::decode::<P::globalStateCall>(&a.unwrap()).unwrap();
    let y = call::decode::<P::globalStateCall>(&raw).unwrap();
    assert_eq!(
        (x.price, x.tick, x.lastFee, x.pluginConfig),
        (y.price, y.tick, y.lastFee, y.pluginConfig)
    );
}
