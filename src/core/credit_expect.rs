//! Файл: core/credit_expect.rs
//! Что делает: ожидаемый приход перевода для сверки события зачисления (решение 76) — так же, как приход в расчёте
//! сделки (costs.rs: «вывод минус затраты вывода в монете»): у биржи — комиссия вывода из справочника сетей ядра
//! (как costs_withdraw.rs), у DEX — затраты вывода из шлюза (налог перевода токена; газ — не в монете, convert::in_coin).
//! Для перегона USDT (usdt_return.rs) то же самое: сумма минус комиссия вывода.
//! Кто вызывает: core/transfers.rs.
//! От чего зависит: dirctx_venue.rs (справочник сетей), convert.rs, deps.rs (шлюз).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};

use super::engine::Core;

/// Ожидаемый приход. Принимает: откуда, монету, сеть отправителя, сумму вывода. Возвращает: сколько должно дойти.
/// Ошибки: текст — комиссия вывода биржи неизвестна, шлюз DEX не дал стоимость вывода, приход не больше нуля.
pub async fn arrival(
    core: &Core,
    from: &VenueId,
    asset: &Asset,
    net: &str,
    qty: Decimal,
) -> Result<Decimal, String> {
    let costs = match super::dirctx_venue::network(core, from, &asset.ticker, net) {
        Some(info) => {
            let fee = info.withdraw_fee.ok_or_else(|| {
                format!(
                    "{}: комиссия вывода {} в сети {net} неизвестна",
                    from.0, asset.ticker
                )
            })?;
            return positive(qty - fee, asset);
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
