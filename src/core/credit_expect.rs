//! Файл: core/credit_expect.rs
//! Что делает: ожидаемый приход перевода для сверки события зачисления (решение 76) — так же, как приход в расчёте
//! сделки (costs.rs: «вывод минус затраты вывода в монете»): у биржи — комиссия вывода из справочника сетей ядра
//! (как costs_withdraw.rs), у DEX — затраты вывода из шлюза (налог перевода токена; газ — не в монете, convert::in_coin).
//! Для перегона USDT (usdt_return.rs) то же самое: сумма минус комиссия вывода. Вывод биржи — ещё и минус налог перевода
//! токена (решение 118, Я-3: transfer_tax.rs — проверка налогов любой DEX по адресу; биржа ↔ биржа без измерения — по
//! cex_cex.untaxed_ok), как в расчёте (costs_withdraw.rs); у нативной монеты налога нет. Налог не известен — только опрос.
//! Ожидание регистрируется, только если у получателя включены события баланса (venue.balance_events) — иначе ни
//! расчёта, ни запроса стоимости вывода.
//! Кто вызывает: core/transfers.rs (ticket).
//! От чего зависит: dirctx_venue.rs (справочник сетей), transfer_tax.rs (налог перевода), convert.rs,
//! credit_watch.rs, deps.rs (шлюз, журнал).
//! Настройки: venue.balance_events (по площадке получателя).

use rust_decimal::Decimal;

use super::settlement_unit::UnitRatio;
use crate::core::types::{Asset, Direction, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_watch::Ticket;
use super::engine::Core;

/// Снимок единиц перед отправкой перевода; берём один раз для обоих способов зачисления.
pub(super) fn units(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
) -> Result<UnitRatio, String> {
    // Служебный возврат котируемой USDT уже ведётся на обеих площадках в USDT.
    // Сравниваем именно контракт котируемой монеты этой сети.
    if asset.contract.is_some()
        && super::registry_chains::usdt_of(core, asset.chain.as_ref()).ok() == asset.contract
    {
        return Ok(UnitRatio::IDENTITY);
    }
    super::settlement_unit::confirmed_cached(
        core,
        &Direction {
            asset: asset.clone(),
            buy_on: from.clone(),
            sell_on: to.clone(),
        },
    )
}

/// Ожидание зачисления, если у получателя включены события баланса и приход посчитан. Принимает: сделку, откуда, куда,
/// монету, сеть отправителя, сумму вывода. Возвращает: ожидание или None (только опрос; причина расчёта — в журнал).
pub async fn ticket(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    net: &str,
    qty: Decimal,
) -> Option<Ticket> {
    let ratio = units(core, from, to, asset);
    ticket_with_units(
        core,
        deal,
        (from, to),
        asset,
        WithdrawalAmount::Budget { network: net, qty },
        ratio.as_ref(),
    )
    .await
}

pub(super) enum WithdrawalAmount<'a> {
    Budget { network: &'a str, qty: Decimal },
    Wire(Decimal),
}

pub(super) async fn ticket_with_units(
    core: &Core,
    deal: &str,
    (from, to): (&VenueId, &VenueId),
    asset: &Asset,
    amount: WithdrawalAmount<'_>,
    unit: Result<&UnitRatio, &String>,
) -> Option<Ticket> {
    let at = At {
        venue: Some(&to.0),
        coin: None,
    };
    if !core
        .inner
        .deps
        .settings
        .flag("venue.balance_events", at)
        .unwrap_or(false)
    {
        return None;
    }
    let arrival = match unit {
        Ok(r) => {
            let sender = match amount {
                WithdrawalAmount::Wire(wire) => after_transfer_tax(core, to, asset, wire),
                WithdrawalAmount::Budget { network, qty } => {
                    sender_amount(core, from, to, asset, network, qty).await
                }
            };
            sender.and_then(|q| r.to_sell(q))
        }
        Err(why) => Err(why.clone()),
    };
    match arrival {
        Ok(a) => Some(super::credit_watch::register(
            core, deal, from, to, asset, a,
        )),
        Err(e) => {
            let text = format!(
                "{deal}: зачисление {} только по опросу статуса — {e}",
                asset.ticker
            );
            core.inner
                .deps
                .log(Level::Info, "core.credit_event", Some(&to.0), text, None)
                .await;
            None
        }
    }
}

/// Ожидаемый приход. Принимает: откуда, куда, монету, сеть отправителя, сумму вывода. Возвращает: сколько должно дойти.
/// Ошибки: текст — комиссия вывода биржи или налог перевода неизвестны, шлюз DEX не дал стоимость вывода, приход не
/// больше нуля.
pub async fn arrival(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    net: &str,
    qty: Decimal,
) -> Result<Decimal, String> {
    let ratio = units(core, from, to, asset)?;
    ratio.to_sell(sender_amount(core, from, to, asset, net, qty).await?)
}

pub(super) async fn sender_amount(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    net: &str,
    qty: Decimal,
) -> Result<Decimal, String> {
    let costs = match super::dirctx_venue::network(core, from, asset, net) {
        Some(info) => {
            if let Some(amounts) =
                super::withdraw_amount::for_network(qty, Some(&info)).map_err(|e| e.to_string())?
            {
                return after_transfer_tax(core, to, asset, amounts.wire);
            }
            // Решение 199: фиксированная + доля суммы вывода (допущение — из суммы; сверка зачисления ловит иное).
            let fee = info.fee_for(qty).ok_or_else(|| {
                format!(
                    "{}: комиссия вывода {} в сети {net} неизвестна",
                    from.0, asset.ticker
                )
            })?;
            // Решение 118 (Я-3): тот же налог, что в расчёте (transfer_tax.rs) — и для получателя-биржи.
            let tax = super::transfer_tax::pct(core, to, asset)?;
            return positive(
                (qty - fee) * (Decimal::ONE - tax / Decimal::ONE_HUNDRED),
                asset,
            );
        }
        None => {
            let gw = core.inner.deps.gw.get(from).await.map_err(|e| e.message)?;
            // Решение 211: площадке — её тикер монеты; затраты — обратно в имя ядра.
            let on = super::venue_ticker::on_venue(core, asset, from);
            let c = gw
                .withdraw_cost(&on, net, qty)
                .await
                .map_err(|e| format!("{}: стоимость вывода: {}", from.0, e.message))?;
            super::venue_ticker::costs_back(core, asset, from, c)
        }
    };
    let in_coin: Decimal = costs
        .iter()
        .filter(|c| !c.in_price && super::convert::in_coin(c, &asset.ticker))
        .map(|c| c.amount)
        .sum();
    positive(qty - in_coin, asset)
}

/// Приход больше нуля. Ошибка: текст.
fn positive(v: Decimal, asset: &Asset) -> Result<Decimal, String> {
    if v > Decimal::ZERO {
        Ok(v)
    } else {
        Err(format!(
            "ожидаемый приход {} не больше нуля: {v}",
            asset.ticker
        ))
    }
}

fn after_transfer_tax(
    core: &Core,
    to: &VenueId,
    asset: &Asset,
    wire: Decimal,
) -> Result<Decimal, String> {
    let tax = super::transfer_tax::pct(core, to, asset)?;
    positive(wire * (Decimal::ONE - tax / Decimal::ONE_HUNDRED), asset)
}

/// Сумма, отправленная биржей после её собственной комиссии, ещё не учитывает
/// удержание контракта. При известном налоге и без измерения размер поступления
/// устанавливается по квитанции; нулевой замер позволяет искать ровно один лог.
pub(super) fn wire_without_tax(
    wire: Decimal,
    measured_tax: Option<Decimal>,
    pinned: Option<UnitRatio>,
) -> Option<Decimal> {
    if measured_tax != Some(Decimal::ZERO) {
        return None;
    }
    pinned?.to_sell(wire).ok()
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    #[test]
    fn sender_wire_does_not_predict_the_wallet_amount_for_taxed_or_unmeasured_tokens() {
        let gross_after_cex_fee = Decimal::from(100);
        let units = Some(UnitRatio::IDENTITY);
        assert_eq!(
            wire_without_tax(gross_after_cex_fee, Some(Decimal::ZERO), units),
            Some(gross_after_cex_fee)
        );
        assert_eq!(
            wire_without_tax(gross_after_cex_fee, Some(Decimal::from(5)), units),
            None
        );
        assert_eq!(wire_without_tax(gross_after_cex_fee, None, units), None);
        assert_eq!(
            wire_without_tax(gross_after_cex_fee, Some(Decimal::ZERO), None),
            None
        );
    }
}
