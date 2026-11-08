//! Файл: core/tax_age.rs
//! Что делает: свежесть замера налогов монеты (решение 234, п. 3): у каждой DEX-стороны направления последняя
//! проверка налогов (CoinCheck.checked_at, адаптер DEX — check/taxes.rs, пишет время удачного замера) не старше
//! tax.max_age_s — иначе расчёта кандидата и пересчёта перед покупкой нет («не торгуем вслепую»: налог мог измениться).
//! Настройка не задана — проверки нет (как до решения 234). Проверки ещё не было или нет времени — решает адаптер
//! (котировка без замера отказывает сама: CoinNotChecked). Продажи уже купленных монет не касается (они не идут через
//! profit.rs).
//! Кто вызывает: core/profit.rs (decide_capped — расчёт кандидата и пересчёт перед покупкой, precheck.rs replan).
//! От чего зависит: market_read.rs (coin_check), trade_ops.rs (вид площадки), settings.
//! Настройки: tax.max_age_s (необязательная).

use chrono::{DateTime, Utc};

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::engine::{Core, rd};

/// Что делает: проверка свежести налогов монеты направления на его DEX-сторонах. Принимает: ядро, направление, момент.
/// Возвращает: Ok — свежие, настройка не задана, монета без адреса или DEX-сторон нет. Ошибки: текст причины.
pub fn check(core: &Core, d: &Direction, now: DateTime<Utc>) -> Result<(), String> {
    let Ok(max) = core
        .inner
        .deps
        .settings
        .integer("tax.max_age_s", At::default())
    else {
        return Ok(());
    };
    let Some(token) = d.asset.bsc_contract else {
        return Ok(());
    };
    for v in [&d.buy_on, &d.sell_on] {
        // Площадки нет в реестре — не здесь: об этом скажет контекст направления (dirctx.rs).
        if !matches!(super::trade_ops::kind(core, v), Ok(VenueKind::Dex)) {
            continue;
        }
        let at = rd(&core.inner.market)
            .coin_check(v, token)
            .and_then(|c| c.checked_at);
        if let Some(age) = stale(at, now, max) {
            return Err(format!(
                "налоги {} на {} измерены {age} с назад — старше tax.max_age_s ({max} с); сделки нет до перепроверки",
                d.asset.ticker, v.0
            ));
        }
    }
    Ok(())
}

/// Что делает: возраст замера, если он старше предела. Принимает: время замера (None — замера нет), момент, предел, с.
/// Возвращает: Some(возраст, с) — старше предела; None — свежий или замера нет. Ошибок нет.
pub fn stale(at: Option<DateTime<Utc>>, now: DateTime<Utc>, max_s: u64) -> Option<i64> {
    let age = (now - at?).num_seconds();
    (age > i64::try_from(max_s).unwrap_or(i64::MAX)).then_some(age)
}
