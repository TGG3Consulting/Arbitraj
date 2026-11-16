//! Файл: tests/bsc_dex_due_read_only_310.rs
//! Что делает: решение 310 (задача t141) — достоверность пулов в check/fresh.rs due() при dex.check_due_read_only:
//! - не задана (и «нет») — как раньше: чтение через update — каждый пул реестра отмечен изменённым для фонового графа
//!   (graph_changes.rs) и версия реестра выросла на число пулов;
//! - «да» — чтение без записи: версия реестра прежняя, отметок изменений нет;
//!
//! в обоих режимах due() отдаёт один и тот же набор пулов (недостоверный пул не берётся), поколения состояния записей
//! (Entry.state_gen) не меняются.
//! Реестр — шесть пулов РЕАЛЬНОЙ записи маршрута CAKE (tests/route_common, routes_2026-10-29). Ответов площадок нет —
//! проверяется только логика реестра и чтения настройки (настоящая SQLite).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{check::fresh, parts, pools::registry}, arb::settings, tests/route_common.

mod route_common;

use std::collections::{HashMap, HashSet};

use serde_json::json;

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::fresh::Fresh;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::listen::Pools;
use arb::venues::bsc_dex::pools::registry::Registry;

use route_common::WBNB_USDT_V2;

/// Поколения состояния всех записей реестра. Ошибок нет.
fn gens(reg: &Registry) -> HashMap<PoolRef, u64> {
    reg.all()
        .into_iter()
        .map(|e| (e.pool.reference.clone(), e.state_gen))
        .collect()
}

/// Один вызов due() при включённом слежении фонового графа (без await между шагами — фоновая задача графа в этом
/// однопоточном рантайме слежение не снимет). Возвращает: набор пулов, рост версии реестра, отметок изменений.
fn run(parts: &Parts, p: &Pools) -> (HashSet<PoolRef>, u64, usize) {
    let reg = &parts.registry;
    let bg = reg.graph_cache().bg();
    bg.changes.track(true);
    let _ = bg.changes.drain();
    let v = reg.version();
    let got: HashSet<PoolRef> = Fresh::default().due(p, |_| false).into_iter().collect();
    let grown = reg.version() - v;
    let marks = bg.changes.pending();
    bg.changes.track(false);
    (got, grown, marks)
}

/// Задать dex.check_due_read_only площадки DEX (None — очистить). Ошибок нет (сбой хранилища — паника теста).
async fn set(parts: &Parts, v: Option<bool>) {
    parts
        .ctx
        .settings
        .set(
            "dex.check_due_read_only",
            Scope::Venue("bsc_dex".into()),
            v.map(|b| json!(b)),
            "тест",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn due_reads_validity_without_registry_writes_only_when_enabled() {
    let parts = route_common::parts().await;
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let p = parts.pools(tx);
    let reg = &parts.registry;
    let n = reg.refs().len();
    assert!(n > 1, "в записи несколько пулов");
    // Один пул недостоверен — due() его не берёт ни в каком режиме.
    let bad = PoolRef::Contract(WBNB_USDT_V2);
    reg.update(&bad, |e| e.valid = false).unwrap();
    let want: HashSet<PoolRef> = reg
        .all()
        .into_iter()
        .filter(|e| e.valid)
        .map(|e| e.pool.reference.clone())
        .collect();
    assert_eq!(want.len(), n - 1);
    assert!(!want.contains(&bad));
    let g0 = gens(reg);

    // Не задана — как раньше: каждый пул отмечен изменённым, версия выросла на число пулов.
    let (got, grown, marks) = run(&parts, &p);
    assert_eq!(got, want);
    assert_eq!(grown, n as u64, "не задана — версия растёт на каждый пул");
    assert_eq!(marks, n, "не задана — каждый пул отмечен изменённым");
    assert_eq!(gens(reg), g0, "поколения не меняются");

    // «Да» — чтение без записи.
    set(&parts, Some(true)).await;
    let (got, grown, marks) = run(&parts, &p);
    assert_eq!(got, want, "тот же набор пулов");
    assert_eq!(grown, 0, "«да» — версия реестра прежняя");
    assert_eq!(marks, 0, "«да» — отметок изменений нет");
    assert_eq!(gens(reg), g0, "поколения не меняются");

    // «Нет» — снова как раньше.
    set(&parts, Some(false)).await;
    let (got, grown, marks) = run(&parts, &p);
    assert_eq!(got, want);
    assert_eq!(grown, n as u64);
    assert_eq!(marks, n);

    // Очищена (не задана) — как раньше.
    set(&parts, None).await;
    let (_, grown, marks) = run(&parts, &p);
    assert_eq!((grown, marks), (n as u64, n));
}
