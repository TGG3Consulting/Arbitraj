//! Файл: tests/bsc_dex_presell_gather_t166.rs
//! Что проверяет: решение 380 (задача t166) — набор источников пополнения кошелька в симуляции продажи до покупки
//! (exec/dry_fund_gather.rs): один источник, как до решения; несколько вместе через узел (один перевод полной суммы на
//! кошелёк — ревью Н4); «вместе мало» — сколько дойдёт до узла и какой объём продажи можно проверить; текст журнала.
//! Записанные данные: строка журнала предпроверки живого бота 17.11.2026 (06:20–09:52 UTC), NXT mexc → bsc_dex: у
//! кошелька 0 единиц, нужно 1549850000000000000000; у держателя Uniswap V4 PoolManager 0x28e2ea09…e9df
//! 205359229175299674167 единиц. Объём запроса NXT в журнале не приведён — 1549.85 (нужно / 10^18) взят как объём, у
//! которого «нужно» ровно столько (налога у записанной строки не видно: пополнение не считалось). Второй источник
//! OTHER и его баланс — условные (чистая логика набора; записанных балансов пулов NXT нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::dry_fund_gather, arb::core::types::TokenTaxes.

use std::str::FromStr;

use alloy_primitives::{Address, U256, address};
use rust_decimal::Decimal;

use arb::venues::bsc_dex::exec::dry_fund_gather::{Fund, Pick, gather, net, qty_share, text};

const V4_MANAGER: Address = address!("28e2ea090877bf75740558f6bfb36a5ffee9e9df");
/// Условный адрес чистой логики (не запись).
const OTHER: Address = address!("3333333333333333333333333333333333333333");

/// Пустая группа источников.
const NONE: &[(Address, U256)] = &[];

fn u(s: &str) -> U256 {
    U256::from_str(s).unwrap()
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn need() -> U256 {
    u("1549850000000000000000")
}

fn holder() -> U256 {
    u("205359229175299674167")
}

#[test]
fn nxt_holder_alone_is_short_and_gives_checkable_qty() {
    // Живой случай: пулов вне маршрута нет, держатель один — набрать нельзя, узлу (держателю) хватит лишь его баланса.
    let h = [(V4_MANAGER, holder())];
    assert_eq!(gather(&[NONE, &h[..]], need(), None), Err(holder()));
    // Без налога дойдёт всё; объём, который можно проверить, — доля объёма запроса, вниз до 18 знаков.
    let can = net(holder(), None).unwrap();
    assert_eq!(can, holder());
    let q = qty_share(d("1549.85"), can, need(), 18).unwrap();
    assert_eq!(q, d("205.359229175299674167"));
    // Ревью t166 Н5: запас на округления (по единице на долю и перевод — exec/dry_fund.rs) — объём меньше, не больше.
    let q2 = qty_share(d("1549.85"), can - U256::from(2u8), need(), 18).unwrap();
    assert!(q2 < q);
}

#[test]
fn single_source_first_then_relay() {
    // Один пул с полной суммой — перевод сразу на кошелёк (как до решения 380), даже если держатель больше.
    let pools = [(OTHER, need())];
    let h = [(V4_MANAGER, need() + U256::from(1u8))];
    assert_eq!(
        gather(&[&pools[..], &h[..]], need(), None),
        Ok(Pick::Direct(OTHER))
    );
    // Пула с полной суммой нет, держатель с ней есть — держатель (как решение 302).
    let small = [(OTHER, u("1"))];
    assert_eq!(
        gather(&[&small[..], &h[..]], need(), None),
        Ok(Pick::Direct(V4_MANAGER))
    );
    // Никто поодиночке не набирает — узел: наибольший источник; остальные переводят ему недостающее, а он ОДНИМ
    // переводом всю сумму на кошелёк (ревью t166 Н4: лимит размера перевода проверяется на полной сумме).
    let rest = need() - holder();
    let pools = [(OTHER, rest + U256::from(5u8))];
    let h = [(V4_MANAGER, holder())];
    assert_eq!(
        gather(&[&pools[..], &h[..]], need(), None),
        Ok(Pick::Relay {
            hub: OTHER,
            feeds: vec![(V4_MANAGER, holder() - U256::from(5u8))]
        })
    );
    // Налог перевода между источниками (наибольший из покупки, продажи, перевода — 10 %): до узла дойдёт 90 % доли —
    // теперь не хватает; узлу достанется свой баланс + 90 % остальных.
    let t = arb::core::types::TokenTaxes {
        buy_pct: d("1"),
        sell_pct: d("10"),
        transfer_pct: d("2"),
    };
    let reach = rest + U256::from(5u8) + holder() * U256::from(9u8) / U256::from(10u8);
    assert_eq!(gather(&[&pools[..], &h[..]], need(), Some(&t)), Err(reach));
    // Один адрес в двух группах считается один раз; нули не берутся.
    let dup = [(V4_MANAGER, holder()), (OTHER, U256::ZERO)];
    assert_eq!(gather(&[&dup[..], &h[..]], need(), None), Err(holder()));
}

#[test]
fn relay_text_names_one_transfer_to_wallet() {
    let fs = [
        Fund {
            src: V4_MANAGER,
            to: Some(OTHER),
            amount: u("5"),
            what: "держателя".into(),
        },
        Fund {
            src: OTHER,
            to: None,
            amount: u("12"),
            what: "пула".into(),
        },
    ];
    let t = text(&fs);
    assert!(t.starts_with("12 единиц одним переводом из пула"), "{t}");
    assert!(t.contains("5 из держателя"), "{t}");
    assert_eq!(text(&fs[1..]), format!("12 единиц из пула {OTHER:#x}"));
}

#[test]
fn net_with_tax_is_inverse_of_gross() {
    let t = arb::core::types::TokenTaxes {
        buy_pct: d("5"),
        sell_pct: Decimal::ZERO,
        transfer_pct: d("1"),
    };
    // 5 % (наибольший из покупки и перевода, как dry_fund::gross): из 1000 дойдёт 950.
    assert_eq!(net(u("1000"), Some(&t)), Some(u("950")));
    // Налог 100 % — ничего не дойдёт.
    let all = arb::core::types::TokenTaxes {
        buy_pct: d("100"),
        ..t.clone()
    };
    assert_eq!(net(u("1000"), Some(&all)), None);
    // Доля никогда не больше объёма запроса и не считается при «нужно 0».
    assert_eq!(qty_share(d("10"), u("20"), u("10"), 18), Some(d("10")));
    assert_eq!(qty_share(d("10"), u("20"), U256::ZERO, 18), None);
}
