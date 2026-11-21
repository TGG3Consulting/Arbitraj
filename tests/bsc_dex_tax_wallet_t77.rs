//! Файл: tests/bsc_dex_tax_wallet_t77.rs
//! Что проверяет: дополнение 10.11 к решениям 247/252 (Тигран: согласовано; задача t77) — перепроверка налогов после
//! отката продажи выхода идёт кошельком сделки. Адреса — из фактов сервера (docs/notes/tax-trace-2026-11-10.md): монета
//! PIG, кошелёк сделки w2; количество на w2 — зачисление сделки 20261109202626272PIG (7065220091 PIG, журнал бота,
//! tests/remeasure_247.rs), PIG — 9 знаков (Transfer «6711960084.822322376» в tax-trace).
//! 1) очередь налогов помнит счёт сделки и id сделки у монеты (coins_by; t78), кнопка «Перепроверить» — без счёта (строка «Пробы»);
//!    очереди крючков и квотера счёт не получают;
//! 2) счёт снимается после итога проверки или «не задана настройка», остаётся при ожидании (повтор тем же кошельком);
//! 3) проба: баланс монеты кошелька до и после покупки в той же симуляции, купленное — прирост (на w2 лежат монеты
//!    сделки — прежде весь баланс считался купленным).
//!
//! Не покрыто: ответ узла eth_simulateV1 от w2 (записанного нет) — итоги balanceOf собраны по ABI из чисел выше.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{recheck, tax_wallet, tax_pass, probe_buy, probe_calls, simulate}.

use std::collections::HashMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address};
use serde_json::json;

use arb::venues::bsc_dex::check::probe_buy::{Head, bought, buy_head};
use arb::venues::bsc_dex::check::probe_calls as pc;
use arb::venues::bsc_dex::check::recheck::{By, Recheck};
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::check::tax_pass::Res;
use arb::venues::bsc_dex::check::tax_wallet::settle;
use arb::venues::bsc_dex::node::abi::IERC20;

const PIG: Address = address!("8850d2c68c632e3b258e612abaa8fada7e6958e5");
const WBNB: Address = address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const W2: Address = address!("a9C3CACff416e644Dec51986DFD0E17Cc2132817");
/// Пара PIG/WBNB (tax-trace) — здесь только метка адресата вызовов квотера и покупки.
const POOL: Address = address!("51dcaf423fe39f620a13379cd26821cf8d433308");

/// Кошелёк сделки w2 и сделка PIG 09.11 (t78: очередь помнит и id сделки).
fn by_w2() -> By {
    By {
        account: "w2".into(),
        deal: "20261109202626272PIG".into(),
    }
}

/// Итог balanceOf (execution-apis CallResult: status 0x1, returnData — uint256 по ABI).
fn balance_result(v: U256) -> CallResult {
    let data = format!("0x{:064x}", v);
    serde_json::from_value(json!({"status": "0x1", "returnData": data, "gasUsed": "0x1"})).unwrap()
}

#[test]
fn tax_queue_keeps_deal_account() {
    let r = Recheck::default();
    r.coins_by(&[PIG], Some(by_w2()));
    r.coins(&[WBNB]);
    let (coins, new_pools, by) = r.taxes.take_all();
    assert!(coins.contains(&PIG) && coins.contains(&WBNB));
    assert!(!new_pools);
    assert_eq!(by.get(&PIG), Some(&by_w2()));
    assert!(!by.contains_key(&WBNB), "кнопка — кошельком строки «Пробы»");
    // Крючки и квотер — без счёта.
    let (h, _, hb) = r.hooks.take_all();
    assert!(h.contains(&PIG) && hb.is_empty());
    let (q, _) = r.quoter.take();
    assert!(q.contains(&PIG));
    // Забрано — очередь пуста.
    let (c2, _, b2) = r.taxes.take_all();
    assert!(c2.is_empty() && b2.is_empty());
}

#[test]
fn account_released_after_verdict() {
    let mut acc = HashMap::from([(PIG, by_w2())]);
    settle(&mut acc, &[(PIG, Res::Wait)]);
    assert!(acc.contains_key(&PIG), "ожидание — повтор тем же кошельком");
    settle(&mut acc, &[(PIG, Res::Ok)]);
    assert!(acc.is_empty());
    let mut acc = HashMap::from([(PIG, by_w2())]);
    settle(&mut acc, &[(PIG, Res::Failed)]);
    assert!(acc.is_empty());
    let mut acc = HashMap::from([(PIG, by_w2())]);
    settle(&mut acc, &[(PIG, Res::Config)]);
    assert!(acc.is_empty());
}

#[test]
fn probe_counts_bought_as_balance_delta() {
    // Начало списка — перевод USDT из пула-источника и одно разрешение покупки.
    let mut list = vec![pc::balance(W2, WBNB, W2); 2];
    let h = buy_head(
        &mut list,
        (W2, PIG),
        (POOL, Bytes::new()),
        (POOL, Bytes::new()),
    );
    assert_eq!(
        h,
        Head {
            quote: 2,
            before: 3,
            buy: 4,
            after: 5
        }
    );
    assert_eq!(list.len(), 6);
    let bal = Bytes::from(IERC20::balanceOfCall { owner: W2 }.abi_encode());
    for i in [h.before, h.after] {
        assert_eq!((list[i].from, list[i].to, &list[i].input), (W2, PIG, &bal));
    }
    assert_eq!(list[h.buy].to, POOL);
    // На w2 — монеты сделки: 7065220091 PIG (9 знаков). Покупка пробы (условная сумма) — прирост.
    let held = U256::from(7_065_220_091u64) * U256::from(1_000_000_000u64);
    let buy = U256::from(1_766_305_285_479_558_519u64);
    let got = bought(&balance_result(held), &balance_result(held + buy));
    assert_eq!(
        got,
        Some(buy),
        "купленное — прирост, не весь баланс кошелька"
    );
    // Кошелёк строки «Пробы» без монет — как прежде (прирост = баланс после).
    assert_eq!(
        bought(&balance_result(U256::ZERO), &balance_result(buy)),
        Some(buy)
    );
    // Баланс после меньше баланса до — купленного нет (не число).
    assert_eq!(
        bought(
            &balance_result(held),
            &balance_result(held - U256::from(1u8))
        ),
        None
    );
}
