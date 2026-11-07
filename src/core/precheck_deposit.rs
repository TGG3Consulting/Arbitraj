//! Файл: core/precheck_deposit.rs
//! Что делает: адрес депозита биржи-получателя — в предпроверке, до покупки (решение 118, Я-5). Раньше адрес впервые
//! запрашивался в transfers.rs уже с монетами на руках: у MEXC адреса для монеты может не быть («создайте в
//! кабинете») — покупка проходила, перевод падал, монета застревала. Теперь ошибка адреса — отказ до покупки, денег
//! не тронуто. Адреса кэшируют адаптеры (по монете и сети) — повторных запросов к бирже нет.
//! Кто вызывает: core/precheck.rs (если B — биржа).
//! От чего зависит: transfers.rs (название сети BSC получателя), deps.rs (шлюз).
//! Настройки: venue.bsc_network — через transfers.rs.

use crate::core::types::Direction;

use super::engine::Core;

/// Есть ли у биржи B адрес депозита монеты в её сети BSC. Принимает: ядро, направление.
/// Ошибки: текст — сети BSC у B нет, адаптер не дал адреса (нет адреса, требуется memo, сбой запроса).
pub async fn ready(core: &Core, d: &Direction) -> Result<(), String> {
    let net = super::transfers::network(core, &d.asset, &d.sell_on)?;
    let gw = core
        .inner
        .deps
        .gw
        .get(&d.sell_on)
        .await
        .map_err(|e| e.message)?;
    // Решение 211: площадке — её тикер монеты.
    let on = super::venue_ticker::on_venue(core, &d.asset, &d.sell_on);
    gw.deposit_address(&on, &net)
        .await
        .map(|_| ())
        .map_err(|e| {
            format!(
                "{}: нет адреса депозита {} в сети {net} — {} (создайте адрес в кабинете биржи)",
                d.sell_on.0, d.asset.ticker, e.message
            )
        })
}
