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

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_watch::Ticket;
use super::engine::Core;

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
    match arrival(core, from, to, asset, net, qty).await {
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
    let costs = match super::dirctx_venue::network(core, from, &asset.ticker, net) {
        Some(info) => {
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
            gw.withdraw_cost(asset, net, qty)
                .await
                .map_err(|e| format!("{}: стоимость вывода: {}", from.0, e.message))?
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
