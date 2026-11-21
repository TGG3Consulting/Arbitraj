//! Файл: core/precheck_deposit.rs
//! Что делает: адрес депозита биржи-получателя — в предпроверке, до покупки (решение 118, Я-5). Раньше адрес впервые
//! запрашивался в transfers.rs уже с монетами на руках: у MEXC адреса для монеты может не быть («создайте в
//! кабинете») — покупка проходила, перевод падал, монета застревала. Теперь ошибка адреса — отказ до покупки, денег
//! не тронуто. Адреса кэшируют адаптеры (по монете и сети) — повторных запросов к бирже нет.
//! Решение 316: монета и сеть запроса — target(); им же пользуется фоновый прогрев адресов (deposit_warm.rs), чтобы
//! первая сделка после перезапуска брала адрес из памяти адаптера, а не запросом к бирже.
//! Решение 379 (t165): общий хвост «(создайте адрес в кабинете биржи)» убран — адаптер биржи с методом создания адрес
//! создаёт сам (KuCoin, Poloniex, Kraken; Gate — сам запрос адреса), что делать при отказе — пишет адаптер.
//! Решение 411 (t196, переделка): адрес из памяти (под отпечатком ключа API) без запроса; запрос к бирже — только по
//! триггеру (нет в памяти, пометка «спросить заново» — core/deposit_proof.rs).
//! Кто вызывает: core/precheck.rs (если B — биржа; при precheck.parallel — одновременно с пересчётом); target — ещё
//! core/deposit_warm_pass.rs; target_on — core/transfers.rs (go: адрес получателя перевода).
//! От чего зависит: transfers.rs (название сети BSC получателя), deps.rs (шлюз), deposit_proof.rs.
//! Настройки: venue.bsc_network — через transfers.rs.

use crate::core::types::{Asset, Direction, VenueId};

use super::engine::Core;

/// Решение 316: что именно запрашивается у биржи B — монета её тикером и название её сети BSC. Тот же ключ, по которому
/// адаптер кэширует адрес (монета, сеть), — им пользуются и предпроверка (ready), и фоновый прогрев (deposit_warm.rs),
/// и перевод (transfers.rs: target_on) — поэтому прогретый адрес берётся сделкой из памяти адаптера.
pub struct Target {
    /// Монета тикером площадки B (решение 211).
    pub on: Asset,
    /// Название сети BSC у площадки B (transfers.rs::network).
    pub net: String,
}

/// Что делает: монета и сеть запроса адреса депозита у площадки B направления (без запроса к бирже).
/// Принимает: ядро, направление. Возвращает: Target.
/// Ошибки: текст — сети BSC у монеты нет (transfers.rs::network).
pub fn target(core: &Core, d: &Direction) -> Result<Target, String> {
    target_on(core, &d.asset, &d.sell_on)
}

/// Что делает: то же для монеты и площадки-получателя без направления — им пользуется перевод (transfers.rs), чтобы
/// ключ адреса прогрева, предпроверки и перевода был один по построению. Принимает: ядро, монету, площадку-получателя.
/// Возвращает: Target. Ошибки: текст — сети BSC у монеты нет (transfers.rs::network).
pub fn target_on(core: &Core, asset: &Asset, to: &VenueId) -> Result<Target, String> {
    let net = super::transfers::network(core, asset, to)?;
    // Решение 211: площадке — её тикер монеты.
    let on = super::venue_ticker::on_venue(core, asset, to);
    Ok(Target { on, net })
}

/// Есть ли у биржи B адрес депозита монеты в её сети BSC. Принимает: ядро, направление.
/// Решение 411 (переделка «по триггеру»): адрес есть в памяти адаптера под текущим ключом API и пометки «спросить
/// заново» нет — шаг без запроса к бирже; иначе — запрос (core/deposit_proof.rs address).
/// Ошибки: текст — сети BSC у B нет, адаптер не дал адреса (нет адреса, требуется memo, сбой запроса).
pub async fn ready(core: &Core, d: &Direction) -> Result<(), String> {
    let Target { on, net } = target(core, d)?;
    let gw = core
        .inner
        .deps
        .gw
        .get(&d.sell_on)
        .await
        .map_err(|e| e.message)?;
    let key: super::deposit_proof::Key = (d.sell_on.clone(), on.ticker.clone(), net.clone());
    super::deposit_proof::address(core, &gw, &key, &on)
        .await
        .map(|_| ())
        .map_err(|e| {
            format!(
                "{}: нет адреса депозита {} в сети {net} — {}",
                d.sell_on.0, d.asset.ticker, e.message
            )
        })
}
