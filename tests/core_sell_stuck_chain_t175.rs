//! Файл: tests/core_sell_stuck_chain_t175.rs
//! Что делает: решение 389 (задача t175) — путь аварии f9634504 17.11.2026 (сделка 20261117143812842GENIUS, bsc_dex →
//! binance, журнал сервера): продавать невыгодно → режим застрявшей монеты «Перегон» (stuck.mode = relay) → продажа
//! здесь по порогу → ордер продажи биржи не исполнился (OrderNotFilled) → повторы 1/3, 2/3, 3/3 через режим
//! застрявшей монеты → core.sell_unsent_here «повторы исчерпаны (stuck.unsent_retries_max = 3)» → продажа здесь по
//! порогу → (на сервере здесь процесс упал: «thread 'tokio-rt-worker' has overflowed its stack»).
//! Стенд — как tests/core_market_sell_268.rs: живое ядро (настоящая SQLite во временной папке) и настоящий адаптер
//! XT.com по HTTP к локальному серверу; справочник пар — живой ответ XT 06.11.2026
//! (tests/data/xt_live_2026-11-06/symbol_pepe.json), стакан — пример официальной документации
//! (tests/data/rest_book_2026-11-09/xt_depth_doc.json). ОТВЕТЫ НА ОРДЕР СОБРАНЫ ПО ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ, А НЕ
//! ЗАПИСАНЫ: постановка — result {orderId} (https://doc.xt.com/docs/spot/Order/SubmitOrder), состояние — поля Query
//! single (https://doc.xt.com/docs/spot/Order/QuerySingle) с state CANCELED и executedQty 0 (IOC снят без исполнения —
//! адаптер отдаёт итог «не исполнилась», ядро — OrderNotFilled, как binance EXPIRED на сервере). Биржа другая, чем на
//! сервере (binance), — путь ядра от площадки не зависит (код OrderNotFilled ставит core/trade_ops.rs). Балансы счетов
//! стенд не отдаёт (404) — сверка остатка (решение 222) идёт «по учёту сделки», как при сбое чтения.
//! Проверяется: (а) поведение — число ордеров (1 + повторы + 1 продажа здесь), порядок строк хронологии и записей
//! журнала (core.stuck, core.sell_unsent_retry, core.sell_unsent_here, core.sell_unsent_exhausted), итог — алерт;
//! (б) стек не растёт с числом повторов: та же цепочка с stuck.unsent_retries_max = 200 проходит в потоке с тем же
//! стеком STACK, что и с 3. До решения 389 каждый повтор вкладывал уровень продажи в стек (core/sell_loop.rs).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues::xt, arb::settings, arb::storage, arb::gateway, arb::secrets.

mod t175_common;

use arb::core::deal::Step;
use serde_json::json;
use t175_common::{ID, big, deal, dir, kinds, lines, orders, saved, set, start};

/// Цепочка аварии с пределом повторов `max`: вход — первая продажа сделки (sell_or_stuck, невыгодно), режим relay
/// (stuck.max_relays = 1 — каталог требует больше нуля; на стенде одна площадка, целей перегона нет → relay.rs «перегон
/// не выгоднее продажи здесь» → relay_here::sell_here, как при исчерпанных перегонах), порог продажи здесь 5 % (убыток
/// ≈ 0,4 %). Пауза повтора 0 — по коду (sell_unsent.rs decide, not_filled_cex), не настройкой. Проверяет (а). Ошибок
/// нет: паника теста.
async fn chain(name: &str, max: u32) {
    let d = dir(name);
    let (core, s, journal, srv) = start(&d).await;
    set(&s, "stuck.mode", json!("relay")).await;
    set(&s, "stuck.max_relays", json!(1)).await;
    set(&s, "stuck.relay_max_loss_pct", json!("5")).await;
    set(&s, "stuck.unsent_retries_max", json!(max)).await;
    arb::core::sell::sell_or_stuck(&core, deal(ID, "200")).await;
    let n = max as usize;
    // Ордера: первая продажа режима + max повторов + одна продажа здесь (решение 229); все лимитные IOC.
    assert_eq!(orders(&srv), n + 2, "{name}");
    let x = saved(&core, ID);
    let t = lines(&x);
    assert_eq!(x.step, Step::WaitingHuman, "{name}: {t:#?}");
    assert_eq!(x.unsent_retries, max + 1, "{name}");
    // Перегона не было: монета всё время на xt, каждый уровень — продажа здесь.
    assert!(!t.iter().any(|l| l.starts_with("Перегон на")), "{name}");
    assert_eq!((x.relays, x.at.0.as_str()), (0, "xt"), "{name}");
    // Хронология по порядку: вход в режим, затем на каждый повтор k — строка повтора и снова вход в режим, затем
    // исчерпание (продажа здесь) и алерт.
    let mut want: Vec<String> = vec!["Застрявшая монета (".into()];
    for k in 1..=max {
        want.push(format!("повтор через режим застрявшей монеты ({k}/{max})"));
        want.push("Застрявшая монета (".into());
    }
    want.push(format!(
        "повторы исчерпаны (stuck.unsent_retries_max = {max}), продажа здесь по порогу"
    ));
    want.push("ждём решения человека".into());
    let got: Vec<&String> = t
        .iter()
        .filter(|l| {
            l.starts_with("Застрявшая монета (")
                || l.contains("повтор через режим застрявшей монеты (")
                || l.contains("продажа здесь по порогу stuck.relay_max_loss_pct (решение 229)")
                || l.contains("ждём решения человека")
        })
        .collect();
    assert_eq!(got.len(), want.len(), "{name}: {got:#?}");
    for (g, w) in got.iter().zip(&want) {
        assert!(g.contains(w.as_str()), "{name}: «{g}» без «{w}»");
    }
    // Журнал по порядку записи: core.stuck, затем [core.sell_unsent_retry, core.stuck] × max, core.sell_unsent_here,
    // core.sell_unsent_exhausted.
    let mut want = vec!["core.stuck"];
    for _ in 0..max {
        want.extend(["core.sell_unsent_retry", "core.stuck"]);
    }
    want.extend(["core.sell_unsent_here", "core.sell_unsent_exhausted"]);
    let keep = [
        "core.stuck",
        "core.sell_unsent_retry",
        "core.sell_unsent_here",
        "core.sell_unsent_exhausted",
    ];
    assert_eq!(kinds(&journal, &keep).await, want, "{name}");
}

/// (а) Значения сервера: stuck.unsent_retries_max = 3 — 5 ордеров, порядок записей как у сделки GENIUS до падения.
#[test]
fn crash_chain_three_retries_same_steps() {
    big(chain("t175-three", 3));
}

/// (б) Та же цепочка с 200 повторами в потоке с тем же стеком: глубина вложенности не зависит от числа повторов.
#[test]
fn crash_chain_many_retries_fits_same_stack() {
    big(chain("t175-many", 200));
}
