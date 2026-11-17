//! Файл: tests/core_close_return_stack_399.rs
//! Что делает: решение 399 (финальное ревью релиза 17.11, Н5) — строгий сторож стека на пути «закрытие сделки →
//! возврат USDT» по образцу сторожей решения 389 (tests/core_sell_loop_t175.rs — поток с малым стеком и однопоточный
//! рантайм): всё тело теста, включая ядро и задачи перегона (tokio::spawn на однопоточном рантайме исполняются в том же
//! потоке), идёт в потоке со стеком WORKER — размер стека рабочего потока tokio по умолчанию (в src
//! thread_stack_size не задан). Путь — настоящий ledger::finish (как у продажи), без вызовов в обход:
//! 1) сделка без группы (exec.top_up не задана — как на сервере): закрытие → usdt_return_deal::prepare → сохранение →
//!    Job::start → перегон (площадок в реестре нет — не состоялся) → «USDT не вернулись»;
//! 2) сделка группы докупки (exec.top_up = да): первая закрытая ждёт (hold_parts), последняя — coin_group::after_close →
//!    coin_group_flush::flush → start_parts → куски выше максимума вывода (usdt_return_pieces::run) → кусок 1 не
//!    дошёл → «не вернулись» на всю сумму.
//! Переполнение стека роняет процесс теста («has overflowed its stack») — это и есть отказ сторожа.
//! Данные: сделка как живая 20261104225504576DOGE (tests/usdt_return_common); максимум вывода USDT BEP20 BingX —
//! запись config/getall 24.10 (tests/data/bingx_2026-10-24/config_getall.json), разбор адаптера BingX. Сеть не
//! нужна: площадок в реестре нет (как tests/core_usdt_return_close.rs). Моков нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::bingx (разбор записи), tests/usdt_return_common.

mod usdt_return_common;

use std::sync::Arc;
use std::time::Duration;

use arb::core::Core;
use arb::core::coin_ride;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::json;

/// Стек потока теста: стек рабочего потока tokio по умолчанию — 2 МиБ (tokio runtime::Builder::thread_stack_size,
/// «The default stack size for spawned threads is 2 MiB»). Параметр теста, не настройка бота.
const WORKER: usize = 2 * 1024 * 1024;

/// Срок ожидания итога перегона (с запасом под нагрузку полного прогона, как tests/core_usdt_return_retry.rs).
const WAIT: Duration = Duration::from_secs(60);

/// Выполняет тело в потоке со стеком WORKER на однопоточном рантайме. Паника тела — паника теста.
fn worker<F>(body: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let run = move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(body);
    };
    let t = std::thread::Builder::new().stack_size(WORKER);
    t.spawn(run).unwrap().join().unwrap();
}

#[test]
fn close_then_return_fits_worker_stack_without_group() {
    worker(async {
        let d = usdt_return_common::dir("stack399-off");
        let (core, _j) = usdt_return_common::start(&d, None).await;
        let mut x = usdt_return_common::doge("s1DOGE");
        arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
        assert!(x.group.is_none());
        assert!(matches!(x.usdt_return, Some(UsdtReturn::Pending { .. })));
        let s = usdt_return_common::wait_state(&core, "s1DOGE", WAIT, |s| {
            matches!(s, UsdtReturn::NotReturned { .. })
        })
        .await;
        match s {
            Some(UsdtReturn::NotReturned { amount, at, .. }) => {
                assert_eq!(
                    (amount, at),
                    (Decimal::new(8310, 2), VenueId("mexc".into()))
                );
            }
            other => panic!("ждали «не вернулись», а {other:?}"),
        }
    });
}

/// Максимум вывода USDT BEP20 BingX и справочник сетей BingX из записи config/getall 24.10 (разбор адаптера).
fn bingx_networks() -> (Decimal, Vec<arb::core::types::CoinNetworkInfo>) {
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/config_getall.json").unwrap();
    let b: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let bingx = VenueId("bingx".into());
    let list = arb::venues::bingx::wallet::parse_networks(&b["data"], "BEP20", &bingx).unwrap();
    let usdt = list.iter().find(|x| x.asset.ticker == "USDT").unwrap();
    (usdt.withdraw_max.unwrap(), list)
}

/// Сделка группы `g` на шаге продажи, проданная на bingx (выручка `proceeds` там), куплено на mexc.
fn member(id: &str, dir: &Direction, g: &str, proceeds: Decimal) -> Deal {
    let mut x = Deal::new(id.into(), dir.clone(), json!(null), Decimal::ONE);
    x.group = Some(Box::new(DealGroup::new(g)));
    x.step = Step::Selling;
    x.at = dir.sell_on.clone();
    x.invested = proceeds - Decimal::ONE;
    x.proceeds = proceeds;
    x
}

/// Ядро с докупкой и данными максимума вывода BingX. Возвращает: ядро, максимум вывода.
async fn group_core() -> (Core, Decimal) {
    let d = usdt_return_common::dir("stack399-group");
    let bingx = Scope::Venue("bingx".into());
    let extra = vec![
        (coin_ride::TOP_UP, Scope::Global, json!(true)),
        ("venue.bsc_network", bingx, json!("BEP20")),
    ];
    let (core, _j) = usdt_return_common::start_with(&d, None, extra).await;
    let (max, list) = bingx_networks();
    let usdt = list.iter().find(|x| x.asset.ticker == "USDT").unwrap();
    *core.inner.usdt.write().unwrap() = Ok(usdt.asset.bsc_contract.unwrap());
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(VenueId("bingx".into()), Arc::new(list));
    (core, max)
}

#[test]
fn close_then_group_return_pieces_fit_worker_stack() {
    worker(async {
        let (core, max) = group_core().await;
        let dir = Direction {
            asset: Asset {
                ticker: "DOGE".into(),
                bsc_contract: None,
            },
            buy_on: VenueId("mexc".into()),
            sell_on: VenueId("bingx".into()),
        };
        // Две сделки группы, каждая с выручкой выше половины максимума: общий возврат — выше максимума, кусками.
        let each = (max * Decimal::new(3, 0) / Decimal::new(4, 0)).round_dp(2);
        for id in ["G", "D2"] {
            let x = member(id, &dir, "G", each);
            arb::core::ledger::save(&core, &x).await;
            coin_ride::board(&core, &dir, id, Some("G".into()));
        }
        let mut x = core.inner.deals.lock().unwrap().get("D2").cloned().unwrap();
        arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
        assert!(
            x.group.as_ref().unwrap().wait.is_some(),
            "первая закрытая ждёт"
        );
        let mut g = core.inner.deals.lock().unwrap().get("G").cloned().unwrap();
        arb::core::ledger::finish(&core, &mut g, Step::Closed, "продано (выгодно)").await;
        let total = each + each;
        let s = usdt_return_common::wait_state(&core, "G", WAIT, |s| {
            matches!(s, UsdtReturn::NotReturned { .. })
        })
        .await;
        match s {
            Some(UsdtReturn::NotReturned { amount, why, .. }) => {
                assert_eq!(amount, total, "кусок 1 не дошёл — не вернулась вся сумма");
                assert!(why.contains("кусок 1 из 2"), "{why}");
            }
            other => panic!("ждали «не вернулись», а {other:?}"),
        }
        let g = core.inner.deals.lock().unwrap().get("G").cloned().unwrap();
        assert!(
            g.timeline
                .iter()
                .any(|(_, t)| t.contains("выводами по очереди")),
            "общий возврат шёл кусками"
        );
        let x = core.inner.deals.lock().unwrap().get("D2").cloned().unwrap();
        assert_eq!(x.group.as_ref().unwrap().shared_in.as_deref(), Some("G"));
    });
}
