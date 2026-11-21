//! Файл: core/delist_check.rs
//! Что делает: проверка делистинга перед сделкой (решение 175, Тигран 05.11.2026). Для КАЖДОЙ стороны направления, где
//! у площадки есть делистинги (только биржи — core/delist.rs опрашивает их одних): монета в делистинге на этой площадке
//! и до прекращения торгов (на стороне покупки A — или до закрытия вывода, что раньше: с A монету выводят) меньше
//! delist.block_hours — сделки нет, причина «ION: делистинг на KuCoin 05.11 00:00 UTC, осталось 3 ч < 48 ч». Время
//! уже прошло или площадка не сообщила время — тоже отказ (не торгуем вслепую).
//! Живой случай 05.11 (сделка 20261105020010981ION, kucoin → mexc): KuCoin снял ION/USDT с торгов, «разница цены»
//! была из-за делистинга. Монеты может не быть в делистинге на MEXC, а на KuCoin — быть: проверяются обе стороны.
//! Биржа отвечает «неизвестно» (нет официального источника) или ответа ещё нет — не блокирует (иначе встанет вся
//! биржа); запись в журнал — в delist.rs. Общая страховка остаётся: пара не торгуется — отсев PairNotTrading.
//! Сопоставление монеты — по тикеру ЭТОЙ площадки (решение 211: монета = адрес, тикеры у бирж разные —
//! venue_ticker::ticker_on). Решение 226 (уточнение Тиграна 08.11.2026): одно правило для ВСЕХ источников —
//! делистинг на бирже X ближе delist.block_hours — монета на X не торгуется; у анонсов время берётся из анонса
//! (gateway/delist_ann.rs), не определено — запись без времени (блок). Метка риска ST (MEXC / KuCoin st, Bybit
//! stTag) блокирует, только если у площадки delist.st_block = да (st_block). Время делистинга уже прошло, а пара в
//! справочнике ещё торгуемая — блок остаётся до явной отмены/переноса в источнике (Тигран: «нет, если явно не
//! получили о продлении или об отмене»): поле времени очистилось или сдвинулось — запись уйдёт сама при следующем
//! обновлении. У монеты несколько записей — берётся строже (Delistings::find).
//! Кто вызывает: core/profit.rs (decide — и очередь кандидатов, и пересчёт предпроверки перед покупкой).
//! От чего зависит: delist.rs (состояние), delist_text.rs (тексты), settings.
//! Настройки: delist.block_hours (по площадке или вся система; не задана — проверка этой площадки выключена, одна
//! запись в журнал); delist.st_block (по площадке, необязательная: не задана или «нет» — метка ST не блокирует).

use std::collections::HashSet;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{DelistItem, Direction, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;

/// Что делает: проверка направления. Принимает: ядро, направление, «сейчас». Возвращает: Ok — ни на одной стороне нет
/// делистинга ближе delist.block_hours. Ошибки: текст причины отказа.
pub async fn check(core: &Core, d: &Direction, now: DateTime<Utc>) -> Result<(), String> {
    for (v, withdraw) in [(&d.buy_on, true), (&d.sell_on, false)] {
        let Some(state) = core.inner.delist.get(v) else {
            continue;
        };
        let Some(got) = state.got else { continue };
        if got.is_unknown() {
            continue;
        }
        // Решение 211: делистинг площадки — по её тикеру монеты; метка ST — только при delist.st_block = да.
        let ticker = super::venue_ticker::ticker_on(core, &d.asset, v);
        let Some(item) = got.find(&ticker, st_block(core, v)) else {
            continue;
        };
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        let hours = match core.inner.deps.settings.decimal("delist.block_hours", at) {
            Ok(h) => h,
            Err(e) => {
                unset_once(core, v, &e.message).await;
                continue;
            }
        };
        verdict(&v.0, item, withdraw, now, hours)?;
    }
    Ok(())
}

/// Что делает: решение по одной стороне (чистая функция). Принимает: площадку, запись делистинга монеты, нужен ли вывод
/// с этой стороны (сторона покупки A), «сейчас», порог в часах. Возвращает: Ok — до делистинга не меньше порога.
/// Ошибки: текст «ION: делистинг на KuCoin 05.11 00:00 UTC, осталось 3 ч < 48 ч», «… время делистинга в прошлом, пара
/// ещё торгуется — блок до явной отмены/переноса», «… время не сообщено (источник)».
pub fn verdict(
    venue: &str,
    item: &DelistItem,
    withdraw: bool,
    now: DateTime<Utc>,
    hours: Decimal,
) -> Result<(), String> {
    use super::delist_text::{hours as h, when};
    let name = crate::notify::human::venue(venue);
    let wd = item.withdraw_end.filter(|_| withdraw);
    let (end, what) = match (item.trading_end, wd) {
        (Some(t), Some(w)) if w < t => (w, "закрытие вывода"),
        (Some(t), _) => (t, "делистинг"),
        (None, Some(w)) => (w, "закрытие вывода"),
        (None, None) if item.st => {
            return Err(format!(
                "{}: метка ST на {name} ({}), delist.st_block = да — не торгуем",
                item.ticker, item.source
            ));
        }
        (None, None) => {
            return Err(format!(
                "{}: делистинг на {name} — время не сообщено ({}), не торгуем вслепую",
                item.ticker, item.source
            ));
        }
    };
    let left = (end - now).num_seconds();
    if left <= 0 {
        // Тигран 08.11: время прошло, а пара ещё в справочнике — блок до явной отмены или переноса в источнике.
        return Err(format!(
            "{}: {what} на {name} {} — время делистинга в прошлом, пара ещё торгуется — блок до явной отмены/переноса ({})",
            item.ticker,
            when(end),
            item.source
        ));
    }
    let limit = hours * Decimal::from(3600);
    if Decimal::from(left) < limit {
        return Err(format!(
            "{}: {what} на {name} {}, осталось {} < {}",
            item.ticker,
            when(end),
            h(left),
            format_args!("{} ч", hours.normalize())
        ));
    }
    Ok(())
}

/// Что делает: блокирует ли метка ST на площадке (delist.st_block). Принимает: ядро, площадку. Возвращает: true —
/// задано «да»; не задано или «нет» — false (ST торгуем). Ошибок нет.
pub fn st_block(core: &Core, v: &VenueId) -> bool {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .flag("delist.st_block", at)
        .unwrap_or(false)
}

/// Площадки, о которых уже записано «delist.block_hours не задана» (одна запись на площадку за запуск).
static UNSET_LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// delist.block_hours не задана — одна запись в журнал на площадку; проверка этой площадки не действует.
/// Принимает: ядро, площадку, текст ошибки настройки. Ошибок нет.
async fn unset_once(core: &Core, v: &VenueId, why: &str) {
    let fresh = {
        let mut g = UNSET_LOGGED.lock().unwrap_or_else(|p| p.into_inner());
        g.get_or_insert_with(HashSet::new).insert(v.0.clone())
    };
    if !fresh {
        return;
    }
    let text = format!(
        "{}: делистинги перед сделкой не проверяются — {why} (решение 175)",
        v.0
    );
    core.inner
        .deps
        .log(Level::Warn, "core.delist", Some(&v.0), text, None)
        .await;
}
