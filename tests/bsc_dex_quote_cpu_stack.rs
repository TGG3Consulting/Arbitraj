//! Файл: tests/bsc_dex_quote_cpu_stack.rs
//! Что делает: решение 301 (дополнение — переполнение стека после слияния t132 с rel/dex-all): замер размера будущего
//! работы котировки DEX, которое quote/cpu.rs run ведёт в потоке блокирующих задач (block_on), и сторож от возврата.
//! (а) голая работа routing::run; (б) после каждой из четырёх обёрток-признаков (quote_stage::final_if,
//! calc_meter::scope_opt, urgency::scope_opt, abandon::scope) с работой ПО ЗНАЧЕНИЮ — ровно то, что лежало на стеке
//! потока счёта в dd74d9b9; (в) cpu::scoped с работой из кучи (Box::pin) — то, что лежит там теперь.
//! Числа пишутся прямо в stderr процесса (std::io::Write на stderr — захват вывода тестов libtest перехватывает только
//! print!/eprint!), поэтому видны в журнале прогона и при успехе теста, без --nocapture.
//! Сторож: размер cpu::scoped не зависит от размера работы (с настоящей работой и с крошечной — одинаков): работа
//! лежит в куче, на стеке — указатель и обёртки. Убрали Box::pin (scoped принимает работу по значению) — размеры
//! разойдутся, тест упадёт с понятным текстом, а не процесс со stack overflow.
//! Будущие не опрашиваются. Цепочка по значению меряется по типу (ret_size: размер результата функции без вызова) —
//! в самом тесте она не собирается, чтобы не переполнить стек потока теста.
//! Данные — запись маршрута CAKE (tests/data/routes_2026-10-29, route_common), как tests/bsc_dex_quote_cpu.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{cpu, routing, abandon}, arb::gateway::{calc_meter, quote_stage,
//! urgency}, tests/route_common.

mod route_common;

use std::future::Future;
use std::io::Write;
use std::mem::size_of_val;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::{Asset, Side, TokenTaxes};
use arb::gateway::{calc_meter, quote_stage, urgency};
use arb::venues::bsc_dex::quote::{Quoting, abandon, cpu, routing};

use route_common::{CAKE, USDT};

/// Входные данные выбора маршрута для покупки CAKE (как в tests/bsc_dex_quote_cpu.rs input).
fn input(q: &Quoting) -> routing::Input {
    let entries = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    let mids = q.mids.active(&q.ctx).into_iter().map(|m| (m, 18)).collect();
    routing::Input {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        side: Side::Buy,
        qty: Decimal::from(5_000),
        coin: CAKE,
        usdt: USDT,
        taxes: TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        },
        coin_dec: 18,
        usdt_dec: 18,
        entries,
        mids,
        gas: U256::from(1_000_000_000u64),
        usdt_ticker: "USDT".into(),
        smart: false,
    }
}

/// Строка в stderr процесса мимо захвата вывода тестов (видна в журнале прогона и при успехе). Ошибок нет.
fn say(s: &str) {
    let mut err = std::io::stderr();
    let _ = err.write_all(format!("{s}\n").as_bytes());
}

/// Цепочка dd74d9b9 по шагам (тот же порядок, что был в quote/cpu.rs run): работа по значению под обёртками.
/// Обычные функции, а не async fn — возвращают ровно будущее обёртки, без своей прослойки. c1 — final_if. Ошибок нет.
fn c1<W: Future>(w: W) -> impl Future<Output = W::Output> {
    quote_stage::final_if(false, w)
}

/// calc_meter::scope_opt над c1. Ошибок нет.
fn c2<W: Future>(w: W) -> impl Future<Output = W::Output> {
    calc_meter::scope_opt(None, c1(w))
}

/// urgency::scope_opt над c2. Ошибок нет.
fn c3<W: Future>(w: W) -> impl Future<Output = W::Output> {
    urgency::scope_opt(None, c2(w))
}

/// abandon::scope над c3 (обёртка t132). Ошибок нет.
fn c4<W: Future>(w: W) -> impl Future<Output = W::Output> {
    abandon::scope(Arc::new(AtomicBool::new(false)), c3(w))
}

/// Размер результата функции — по типу, без вызова: цепочка по значению в самом тесте не собирается (она и
/// переполняла стек потока счёта; поток теста тоже 2 МиБ). Ошибок нет.
fn ret_size<A, R>(_: fn(A) -> R) -> usize {
    size_of::<R>()
}

/// Размеры цепочки по значению над работой типа W: после final_if, calc_meter, urgency, abandon. Ошибок нет.
fn chain_sizes<W: Future>(_: &W) -> [usize; 4] {
    [
        ret_size(c1::<W>),
        ret_size(c2::<W>),
        ret_size(c3::<W>),
        ret_size(c4::<W>),
    ]
}

#[tokio::test]
async fn future_sizes_t132() {
    let p = route_common::parts().await;
    let q = p.quoting();
    // (а) голая работа (собирается, не опрашивается).
    let work = routing::run(q.clone(), input(&q));
    let bare = size_of_val(&work);
    // (б) цепочка обёрток с работой по значению — как в quote/cpu.rs run до 8bef5371 (dd74d9b9).
    let [s1, s2, s3, s4] = chain_sizes(&work);
    // (в) то, что run ведёт сейчас: cpu::scoped с работой из кучи.
    let (_g1, flag1) = abandon::guard();
    let boxed = cpu::scoped(flag1, None, None, false, Box::pin(work));
    let now = size_of_val(&boxed);
    // Та же цепочка с крошечной работой: размер cpu::scoped не должен зависеть от работы.
    let (_g2, flag2) = abandon::guard();
    let tiny = cpu::scoped(flag2, None, None, false, Box::pin(std::future::ready(())));
    let tiny_size = size_of_val(&tiny);
    let ptr = size_of::<usize>();
    let text = format!(
        "t132 размер будущего работы котировки, байт: routing::run {bare}; по значению (dd74d9b9): \
         + final_if {s1}, + calc_meter::scope_opt {s2}, + urgency::scope_opt {s3}, + abandon::scope {s4}; \
         сейчас cpu::scoped (Box::pin) {now}; cpu::scoped с крошечной работой {tiny_size}; указатель {ptr}"
    );
    say(&text);
    // Сторож: работа в куче — на стеке потока счёта указатель и обёртки, размер от работы не зависит.
    assert_eq!(
        now, tiny_size,
        "будущее потока счёта зависит от размера работы — работа снова лежит по значению (нет Box::pin?): {text}"
    );
    // Работа из кучи не крупнее цепочки по значению (иначе замер не показывает выигрыша — нужен иной разбор).
    assert!(now < s4, "{text}");
}
