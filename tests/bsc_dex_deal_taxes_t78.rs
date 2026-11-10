//! Файл: tests/bsc_dex_deal_taxes_t78.rs
//! Что проверяет: дополнение 10.11 к решениям 247/252 (задача t78, Тигран: согласовано) — налоги, перепроверенные
//! кошельком сделки, хранятся отдельно (check/deal_taxes.rs) и действуют только для продажи ЭТОЙ сделки:
//! 1) общие налоги монеты и пула (кошелёк «Пробы») не меняются; копия продажи сделки (признак продажи выхода с id
//!    сделки — gateway/exit_scope.rs, Checks::for_exit) видит налоги сделки поверх общих, другая сделка, другой счёт,
//!    вне признака и признак без id — общие; пул, который перепроверка не пробовала, — общий;
//! 2) срок жизни: продажа выхода сделки целиком — записи сняты; частично или не исполнилась — остаются; вывод монеты
//!    сделки с кошелька — снята её запись.
//!
//! Числа — PIG 09.11 (сделка 20261109202626272PIG, кошелёк w2, docs/notes/tax-trace-2026-11-10.md): модель налога
//! кошелька «Пробы» 4.0003 %, налог перевода в сети — ровно 5.0 % (журнал бота и квитанции, tests/remeasure_247.rs).
//! Итог перепроверки кошельком w2 здесь взят равным налогу в сети (5.0 %) — записанного ответа eth_simulateV1 от w2 нет.
//! Не покрыто: сама проба кошельком сделки на узле (tax_wallet::deal_check — записанного ответа нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{state, deal_taxes}, arb::gateway::exit_scope, arb::core::types.

use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::{Address, address};
use chrono::Utc;
use rust_decimal::Decimal;

use arb::core::types::{
    Asset, CheckState, CoinCheck, PoolRef, Side, TokenTaxes, TradeRequest, TradeResult,
    TradeStatus, VenueId, WithdrawRequest,
};
use arb::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use arb::gateway::exit_scope;
use arb::venues::bsc_dex::check::deal_taxes::DealTax;
use arb::venues::bsc_dex::check::state::{Checks, PoolTaxes};

const PIG: Address = address!("8850d2c68c632e3b258e612abaa8fada7e6958e5");
/// Пара PIG/WBNB (tax-trace) — пул, который перепроверка пробовала.
const POOL: Address = address!("51dcaf423fe39f620a13379cd26821cf8d433308");
/// Условный второй пул PIG — перепроверка его не пробовала (адрес — метка теста).
const OTHER: Address = address!("0000000000000000000000000000000000000001");
const DEAL: &str = "20261109202626272PIG";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn pig() -> Asset {
    Asset {
        ticker: "PIG".into(),
        bsc_contract: Some(PIG),
    }
}

fn taxes(sell: &str) -> TokenTaxes {
    TokenTaxes {
        buy_pct: d(sell),
        sell_pct: d(sell),
        transfer_pct: d(sell),
    }
}

fn check(sell: &str) -> CoinCheck {
    CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: pig(),
        taxes: CheckState::Known(taxes(sell)),
        source: "eth_simulateV1".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    }
}

fn pool_taxes(sell: &str) -> PoolTaxes {
    PoolTaxes {
        buy_pct: d(sell),
        sell_pct: Some(d(sell)),
        transfer_pct: d(sell),
    }
}

/// Общие итоги (кошелёк «Пробы»): налог PIG 4.0003 % в обоих пулах; перепроверка кошельком w2 сделки PIG — 5.0 %.
fn setup() -> Checks {
    let c = Checks::default();
    c.set_coin(check("4.0003"));
    for p in [POOL, OTHER] {
        c.update_pool(&PoolRef::Contract(p), |f| {
            f.taxes = Some(pool_taxes("4.0003"))
        });
    }
    let t = DealTax {
        account: "w2".into(),
        check: check("5.0"),
        pools: HashMap::from([(PoolRef::Contract(POOL), Some(pool_taxes("5.0")))]),
    };
    c.put_deal(DEAL, PIG, t);
    c
}

fn sell_pct(c: &Checks) -> Decimal {
    match c.coin(&PIG).map(|x| x.taxes) {
        Some(CheckState::Known(t)) => t.sell_pct,
        other => panic!("{other:?}"),
    }
}

fn pool_sell(c: &Checks, p: Address) -> Option<Decimal> {
    c.pool(&PoolRef::Contract(p)).taxes.and_then(|t| t.sell_pct)
}

#[tokio::test]
async fn deal_taxes_only_for_this_deal_sale() {
    let c = setup();
    // Общие — кошелька «Пробы», как до t77.
    assert_eq!(sell_pct(&c), d("4.0003"));
    assert_eq!(pool_sell(&c, POOL), Some(d("4.0003")));
    // Продажа выхода этой сделки кошельком w2 — налоги сделки; пул, который не пробовался, — общий.
    let s = exit_scope::scope(Some(DEAL), async { c.for_exit(Some("w2")) }).await;
    assert_eq!(sell_pct(&s), d("5.0"));
    assert_eq!(pool_sell(&s, POOL), Some(d("5.0")));
    assert_eq!(pool_sell(&s, OTHER), Some(d("4.0003")));
    // Общие от копии не изменились.
    assert_eq!(sell_pct(&c), d("4.0003"));
    assert_eq!(pool_sell(&c, POOL), Some(d("4.0003")));
    // Другая сделка, другой счёт, вне признака, признак без id — общие.
    let other = exit_scope::scope(Some("другая"), async { c.for_exit(Some("w2")) }).await;
    assert_eq!(sell_pct(&other), d("4.0003"));
    let acc = exit_scope::scope(Some(DEAL), async { c.for_exit(Some("w1")) }).await;
    assert_eq!(sell_pct(&acc), d("4.0003"));
    assert_eq!(sell_pct(&c.for_exit(Some("w2"))), d("4.0003"));
    let no_id = exit_scope::scope(None, async { c.for_exit(Some("w2")) }).await;
    assert_eq!(sell_pct(&no_id), d("4.0003"));
    // Признак продажи выхода — и без id (пороги входа не применяются, решение 247 п. В).
    assert!(exit_scope::scope(None, async { exit_scope::is_exit() }).await);
    assert!(!exit_scope::is_exit());
    // Снимок котировки (решение 239) копии продажи сделки — с теми же налогами сделки.
    let pinned = s.pinned(&[PIG], [&PoolRef::Contract(POOL)]);
    assert_eq!(sell_pct(&pinned), d("5.0"));
}

fn sale(qty: &str) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: pig(),
        side: Side::Sell,
        qty: d(qty),
        worst_price: Decimal::ZERO,
        deal_id: DEAL.into(),
        route: None,
        exit_position: true,
        remeasure: None,
    }
}

fn result(req: &TradeRequest, status: TradeStatus, filled: &str) -> GatewayResult<TradeResult> {
    Ok(TradeResult {
        request: req.clone(),
        status,
        filled_qty: d(filled),
        avg_price: Decimal::ZERO,
        fees: Vec::new(),
        venue_ref: String::new(),
        chain: None,
        finished_at: Utc::now(),
        executor: None,
    })
}

#[test]
fn deal_taxes_released_when_coins_leave_wallet() {
    let c = setup();
    let req = sale("7065220091");
    // Не исполнилась (ошибка) или частично — монеты на кошельке, запись остаётся.
    let err: GatewayResult<TradeResult> = Err(GatewayError::new(
        ErrorCode::NoRoute,
        None,
        "minOut".to_string(),
    ));
    c.forget_after_sale(&req, &err);
    c.forget_after_sale(&req, &result(&req, TradeStatus::PartiallyFilled, "1"));
    assert!(c.deal_tax(DEAL, &PIG).is_some());
    // Покупка — не продажа выхода.
    let mut buy = sale("1");
    buy.side = Side::Buy;
    buy.exit_position = false;
    c.forget_after_sale(&buy, &result(&buy, TradeStatus::Filled, "1"));
    assert_eq!(c.deal_count(), 1);
    // Продано целиком — снята.
    c.forget_after_sale(&req, &result(&req, TradeStatus::Filled, "7065220091"));
    assert_eq!(c.deal_count(), 0);
    // Вывод монеты сделки с кошелька (перегон) — снята; вывод другой монеты — нет; ошибка вывода — нет.
    let c = setup();
    let w = |a: Asset| WithdrawRequest {
        venue: VenueId("bsc_dex".into()),
        asset: a,
        network: "BSC".into(),
        address: String::new(),
        amount: d("1"),
        deal_id: DEAL.into(),
    };
    let usdt = Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(OTHER),
    };
    c.forget_after_withdraw(&w(usdt), &Ok(()));
    let failed: GatewayResult<()> = Err(GatewayError::new(
        ErrorCode::NoRoute,
        None,
        "отказ".to_string(),
    ));
    c.forget_after_withdraw(&w(pig()), &failed);
    assert_eq!(c.deal_count(), 1);
    c.forget_after_withdraw(&w(pig()), &Ok(()));
    assert_eq!(c.deal_count(), 0);
}
