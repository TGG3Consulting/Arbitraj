//! Файл: core/precheck.rs
//! Что делает: предпроверка сразу перед покупкой (шаг 5 ТЗ): стоп-условия (risk.rs); статусы вывода на A
//! и депозита на B — перечитываются напрямую из API бирж; покупка через API на A и продажа на B — по свежему справочнику
//! пар (pairs_fresh.rs); BNB на газ у DEX-сторон не ниже
//! risk.min_bnb_balance; затем полный расчёт заново (profit.rs) по свежим стаканам и пулам.
//! Кто вызывает: executor.rs.
//! От чего зависит: risk.rs, profit.rs, convert.rs, trade_ops.rs (вид площадки), deps.rs (шлюз).
//! Настройки: risk.min_bnb_balance, venue.symbols_stale_s (+ всё, что читают risk.rs и profit.rs).

use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::costs::Calc;
use super::engine::{Core, wr};
use super::profit::{self, Verdict};

/// Статус ввода или вывода монеты по сети BSC прямо из API биржи. Ошибка — текст причины.
async fn transfer_open(
    core: &Core,
    v: &VenueId,
    d: &Direction,
    withdraw: bool,
) -> Result<(), String> {
    let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    let list = gw
        .coin_networks()
        .await
        .map_err(|e| format!("{}: справочник сетей: {}", v.0, e.message))?;
    // Нативная монета ↔ обёртка (решение 62): тот же справочник, что у ядра (native.rs).
    let list = super::native::map(core, v, list);
    let open = list
        .iter()
        .find(|i| {
            i.asset.ticker == d.asset.ticker
                && i.asset.bsc_contract.is_some()
                && i.asset.bsc_contract == d.asset.bsc_contract
        })
        .map(|i| {
            if withdraw {
                i.withdraw_enabled
            } else {
                i.deposit_enabled
            }
        });
    wr(&core.inner.networks).insert(v.clone(), list);
    core.request_rebuild();
    match open {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "{}: {} {} закрыт",
            v.0,
            d.asset.ticker,
            if withdraw { "вывод" } else { "ввод" }
        )),
        None => Err(format!(
            "{}: у {} нет сети BSC с тем же адресом контракта",
            v.0, d.asset.ticker
        )),
    }
}

/// Сторона сделки по справочнику пар биржи (pairs_fresh.rs): на A (buy = true) покупка через API разрешена, на B —
/// продажа; справочник не старше venue.symbols_stale_s. Направление могло быть поставлено до смены флагов —
/// здесь оно отсекается до ордера. Ошибка — текст причины.
fn pair_side(core: &Core, v: &VenueId, d: &Direction, buy: bool) -> Result<(), String> {
    let limit = super::pairs_fresh::limit(&core.inner.deps.settings, v);
    let market = super::engine::rd(&core.inner.market);
    super::pairs_fresh::side(&market, v, &d.asset.ticker, buy, &limit, chrono::Utc::now())
}

/// BNB на газ у DEX-площадки: свободный нативный BNB ≥ резерв risk.min_bnb_balance + газ транзакций сделки на ней
/// (need; сверка балансов — 0). Ошибка — текст (мало BNB, резерв не задан или баланс не получен).
pub async fn bnb_ok(core: &Core, v: &VenueId, need: Decimal) -> Result<(), String> {
    let min = core
        .inner
        .deps
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| e.message)?;
    let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    let b = gw
        .balances()
        .await
        .map_err(|e| format!("{}: балансы: {}", v.0, e.message))?;
    let have: Decimal = b
        .iter()
        .filter(|x| x.asset == super::convert::GAS_TICKER)
        .map(|x| x.free)
        .sum();
    if have < min + need {
        return Err(format!(
            "{}: BNB на газ {have} меньше минимума {min} + газ сделки {need}",
            v.0
        ));
    }
    Ok(())
}

/// Предпроверка. Возвращает: свежий расчёт для сделки. Ошибки: текст причины отказа.
pub async fn run(core: &Core, d: &Direction) -> Result<Calc, String> {
    super::risk::can_open(core, d)?;
    for (v, withdraw) in [(&d.buy_on, true), (&d.sell_on, false)] {
        match super::trade_ops::kind(core, v)? {
            VenueKind::Cex => {
                transfer_open(core, v, d, withdraw).await?;
                pair_side(core, v, d, withdraw)?;
            }
            VenueKind::Dex => {
                if let Err(e) = bnb_ok(core, v, Decimal::ZERO).await {
                    core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
                    return Err(e);
                }
            }
        }
    }
    let c = match profit::decide(core, d).await {
        Verdict::Accept(c) => c,
        Verdict::Reject { reason, .. } => return Err(format!("пересчёт перед покупкой: {reason}")),
    };
    // Решение 62: газ всех транзакций сделки на DEX — сверх резерва, иначе после покупки BNB перевод на биржу
    // упадёт с InsufficientBalance и монета застрянет. A — покупка и перевод, B — продажа и возврат USDT.
    for (v, stages) in [
        (&d.buy_on, ["A", "перевод"]),
        (&d.sell_on, ["B", "возврат USDT"]),
    ] {
        if super::trade_ops::kind(core, v)? != VenueKind::Dex {
            continue;
        }
        let need: Decimal = c
            .costs
            .iter()
            .filter(|l| stages.contains(&l.stage) && l.asset == super::convert::GAS_TICKER)
            .filter(|l| super::convert::is_gas(l.kind))
            .map(|l| l.amount)
            .sum();
        if let Err(e) = bnb_ok(core, v, need).await {
            core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
            return Err(e);
        }
    }
    Ok(c)
}
