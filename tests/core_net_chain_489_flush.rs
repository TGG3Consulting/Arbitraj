//! Файл: tests/core_net_chain_489_flush.rs
//! Что делает: решение 489 (ревью 4) — общий возврат USDT группы докупки (решение 394, coin_group_flush.rs flush) не
//! теряет держателя сети площадки возврата и не задваивает сумму. Настоящий путь ядра (t489_common bare, сделки —
//! ledger::save): A — первая сделка группы, закрыта, выручка ждёт общего возврата с arb_dex; B — вложенная, идёт
//! (запись в идущих, Inner.rides), закрывается последней → coin_group::after_close → flush. Наблюдаемые состояния учёта —
//! события сохранения ядра (CoreEvent::Deal, ledger.rs save): первым сохраняется носитель A с частью «идёт» на всю сумму
//! (держатель сети есть), потом B без ожидания (общий возврат в сделке A); итог в учёте: ожиданий нет, часть одна — сумма
//! не задвоена. Промежуточное состояние учёта памяти между двумя сохранениями (носитель и копии вносятся одним захватом
//! замка до сохранений) снаружи без точек перехвата не наблюдается — проверяется порядок сохранений и итог.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (coin_group, coin_key, coin_ride, deal, deal_group, ledger, net_hold, view), t489_common.
//! Моков нет.

mod t489_common;

use rust_decimal::Decimal;

use arb::core::coin_key::CoinKey;
use arb::core::coin_ride::Ride;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::net_hold::holds_net;
use arb::core::types::DexAccountId;
use arb::core::view::CoreEvent;
use t489_common::deals::new_deal;
use t489_common::{arb_chain, bare, v};

/// Сделка группы `A` на arb_dex: шаг, ожидание общего возврата (откуда arb_dex, 1 USDT) или нет. Ошибок нет.
fn member(id: &str, step: Step, wait: bool) -> Deal {
    let mut d = new_deal(id, "arb_dex", Some(arb_chain()));
    d.step = step;
    d.dex_account = Some(DexAccountId("w489".into()));
    d.group = Some(Box::new(DealGroup {
        id: "A".into(),
        wait: wait.then(|| (v("arb_dex"), Decimal::ONE)),
        ..Default::default()
    }));
    d
}

/// Ожидания и части возврата сделки — сумма. Ошибок нет.
fn due(d: &Deal) -> Decimal {
    let wait = d.group.as_ref().and_then(|g| g.wait.as_ref());
    let mut sum = wait.map_or(Decimal::ZERO, |w| w.1);
    for p in arb::core::usdt_return_parts::all(d) {
        if let arb::core::usdt_return_state::UsdtReturn::Pending { amount, .. } = p {
            sum += *amount;
        }
    }
    sum
}

#[tokio::test]
async fn group_flush_keeps_holder_and_sum() {
    let core = bare("net-chain-489-flush").await;
    let arb = v("arb_dex");
    let mut a = member("A", Step::Closed, true);
    arb::core::ledger::save(&core, &a).await;
    let b = member("B", Step::Selling, false);
    arb::core::ledger::save(&core, &b).await;
    let ride = Ride {
        deal_id: "B".into(),
        buy_on: v("bingx"),
        sell_on: arb.clone(),
        group: Some("A".into()),
    };
    let key = CoinKey::of(&b.direction.asset);
    core.inner.rides.lock().unwrap().insert(key, vec![ride]);
    // A закрыта первой: её часть в пуле, B ещё идёт — общего возврата нет.
    arb::core::coin_group::after_close(&core, &mut a).await;
    assert!(a.group.as_ref().is_some_and(|g| g.wait.is_some()));
    // B закрыта последней — общий возврат группы.
    let mut b = member("B", Step::Closed, true);
    arb::core::ledger::save(&core, &b).await;
    let mut rx = core.subscribe();
    arb::core::coin_group::after_close(&core, &mut b).await;
    let mut saved: Vec<Deal> = vec![];
    while let Ok(ev) = rx.try_recv() {
        if let CoreEvent::Deal(x) = ev {
            saved.push(serde_json::from_value(x).unwrap());
        }
    }
    assert!(saved.len() >= 2, "сохранены носитель и B");
    assert_eq!(saved[0].id, "A", "носитель — первым");
    assert!(holds_net(&saved[0], &arb), "держатель — часть «идёт»");
    assert_eq!(due(&saved[0]), Decimal::TWO, "у носителя — вся сумма");
    assert_eq!(saved[1].id, "B");
    let bg = saved[1].group.as_ref().unwrap();
    assert!(bg.wait.is_none() && bg.shared_in.as_deref() == Some("A"));
    // Итог в учёте: сумма к возврату — ровно сумма частей группы, без задвоения.
    let deals = core.inner.deals.lock().unwrap();
    let total = due(&deals["A"]) + due(&deals["B"]);
    assert_eq!(total, Decimal::TWO, "сумма не задвоена");
}
