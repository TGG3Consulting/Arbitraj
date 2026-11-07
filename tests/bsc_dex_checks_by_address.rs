//! Файл: tests/bsc_dex_checks_by_address.rs
//! Что делает: проверяет решение 211 (фаза 2b) в DEX-адаптере — налоги монеты и газ перевода хранятся по АДРЕСУ
//! контракта BSC и находятся при другом показном тикере той же монеты (живой случай 07.11: bingx NMT = gate NETMIND,
//! один адрес 0x03aa6298…e8d6 — из записанного справочника gate, tests/data/cex_samples_2026_11_04).
//! Кто вызывает: cargo test.
//! От чего зависит: venues/bsc_dex/check/state.rs (Checks), core/types (Asset, CoinCheck).

use alloy_primitives::{Address, address};
use rust_decimal::Decimal;

use arb::core::types::{Asset, CheckState, CoinCheck, TokenTaxes, VenueId};
use arb::venues::bsc_dex::check::state::Checks;

/// Адрес NMT/NETMIND в сети BSC — из записанного ответа gate (gate_spot_currencies.json, поле addr).
const NMT: Address = address!("0x03aa6298f1370642642415edc0db8b957783e8d6");

/// Монета с тикером `t` и адресом `a`. Ошибок нет.
fn asset(t: &str, a: Option<Address>) -> Asset {
    Asset {
        ticker: t.into(),
        bsc_contract: a,
    }
}

#[test]
fn taxes_and_gas_found_by_address_under_other_ticker() {
    let checks = Checks::default();
    let taxes = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ONE,
        transfer_pct: Decimal::ZERO,
    };
    // Проверку записали, когда показное имя монеты было «NMT» (тикер BingX).
    checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: asset("NMT", Some(NMT)),
        taxes: CheckState::Known(taxes.clone()),
        source: "eth_call".into(),
        checked_at: None,
        wait: None,
    });
    checks.set_transfer_gas(NMT, 51_000);
    checks.set_native_transfer_gas(NMT, 21_000);
    // Показное имя сменилось на «NETMIND» (тикер Gate) — налоги и газ те же: ключ — адрес.
    let renamed = asset("NETMIND", Some(NMT));
    let got = checks
        .coin_of(&renamed)
        .expect("налоги не найдены по адресу");
    assert_eq!(got.taxes, CheckState::Known(taxes));
    assert_eq!(
        checks.coin(&NMT).map(|c| c.asset.ticker),
        Some("NMT".into())
    );
    assert_eq!(checks.transfer_gas(&NMT), Some(51_000));
    assert_eq!(checks.native_transfer_gas(&NMT), Some(21_000));
    // Тот же тикер, но другой адрес — другая монета: налогов нет.
    let other = asset("NMT", Some(Address::repeat_byte(0x11)));
    assert!(checks.coin_of(&other).is_none());
    // Монета без адреса — проверка не хранится и не находится (тикер ключом не служит).
    checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: asset("NOADDR", None),
        taxes: CheckState::NotChecked,
        source: "eth_call".into(),
        checked_at: None,
        wait: None,
    });
    assert!(checks.coin_of(&asset("NOADDR", None)).is_none());
}
