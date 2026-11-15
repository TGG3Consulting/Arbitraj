//! Файл: tests/bsc_dex_quote_cpu_depth.rs
//! Что делает: решение 301 (дополнение — стек): замер ГЛУБИНЫ стека потока счёта котировки DEX до начала работы —
//! тот же путь, что quote/cpu.rs run: spawn_blocking → Handle::block_on(цепочка обёрток-признаков над работой).
//! Зонд — async-работа, которая держит настоящее будущее routing::run по значению (тот же размер, не опрашивается)
//! и при первом опросе записывает адрес своей локальной переменной. Верх — адрес локальной переменной в начале
//! замыкания потока счёта. Разность = стек, занятый вызовом обёрток, block_on и входом в рантайм ДО начала работы:
//! (а) как в dd74d9b9 — четыре обёртки по значению собираются в потоке счёта; (б) как сейчас — работа из кучи
//! (Box::pin до spawn_blocking), cpu::scoped. Над «верхом» не меряются: кадры tokio потока блокирующих задач и
//! само замыкание (держит зонд по значению в (а), указатель в (б)).
//! Глубину самой работы (синхронный счёт графа/котировки внутри опроса) этим тестом измерить нельзя без правки
//! боевого кода (точки замера внутри работы нет) или unsafe — здесь не меряется.
//! Числа пишутся прямо в stderr процесса (мимо захвата вывода libtest) — видны в журнале прогона и при успехе.
//! Данные — запись маршрута CAKE (tests/data/routes_2026-10-29, route_common).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{cpu, routing, abandon}, arb::gateway::{calc_meter, quote_stage,
//! urgency}, tests/route_common.

mod route_common;

use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloy_primitives::U256;
use rust_decimal::Decimal;
use tokio::runtime::Handle;

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

/// Адрес локальной переменной своего кадра — текущая глубина стека (стек растёт вниз). Без unsafe. Ошибок нет.
#[inline(never)]
fn sp() -> usize {
    let x = 0u8;
    let p: *const u8 = std::hint::black_box(&x);
    p.addr()
}

/// Зонд: держит работу по значению (размер — как у работы), при первом опросе пишет глубину стека. Ошибок нет.
async fn probe<W>(work: W, low: Arc<AtomicUsize>) {
    low.store(sp(), Ordering::SeqCst);
    let _ = std::hint::black_box(&work);
}

/// Как в dd74d9b9: четыре обёртки по значению собираются в потоке счёта и уходят в block_on. Ошибок нет.
#[inline(never)]
fn by_value<F: Future>(rt: &Handle, flag: Arc<AtomicBool>, work: F) -> F::Output {
    rt.block_on(abandon::scope(
        flag,
        urgency::scope_opt(
            None,
            calc_meter::scope_opt(None, quote_stage::final_if(false, work)),
        ),
    ))
}

/// Как сейчас (quote/cpu.rs run): работа из кучи, cpu::scoped. Ошибок нет.
#[inline(never)]
fn from_heap<F: Future>(rt: &Handle, flag: Arc<AtomicBool>, work: Pin<Box<F>>) -> F::Output {
    rt.block_on(cpu::scoped(flag, None, None, false, work))
}

/// Глубина стека до начала работы: (байт от верха замыкания до первого опроса зонда, размер зонда). Ошибок нет.
async fn depth(q: &Quoting, heap: bool) -> (usize, usize) {
    let low = Arc::new(AtomicUsize::new(0));
    let pr = probe(routing::run(q.clone(), input(q)), low.clone());
    let size = std::mem::size_of_val(&pr);
    let rt = Handle::current();
    let (_guard, flag) = abandon::guard();
    let top = if heap {
        let pr = Box::pin(pr);
        tokio::task::spawn_blocking(move || {
            let top = sp();
            from_heap(&rt, flag, pr);
            top
        })
        .await
        .unwrap()
    } else {
        tokio::task::spawn_blocking(move || {
            let top = sp();
            by_value(&rt, flag, pr);
            top
        })
        .await
        .unwrap()
    };
    let low = low.load(Ordering::SeqCst);
    assert_ne!(low, 0, "зонд не опрошен");
    (top.saturating_sub(low), size)
}

#[tokio::test]
async fn stack_depth_before_work_t132() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let work = std::mem::size_of_val(&routing::run(q.clone(), input(&q)));
    let (x, size) = depth(&q, false).await;
    let (y, _) = depth(&q, true).await;
    let text = format!(
        "t132 стек до начала работы: по значению {x} байт, из кучи {y} байт; стек потока 2 МиБ (2097152 байт, \
         по умолчанию tokio, runtime/builder.rs); работе остаётся сейчас не больше {} байт; \
         размер зонда {size} (routing::run {work})",
        2_097_152usize.saturating_sub(y)
    );
    say(&text);
    // Только проверка, что замер состоялся (оба зонда опрошены — проверено в depth); числа — в журнал как есть.
    assert!(x > 0 && y > 0, "{text}");
}
