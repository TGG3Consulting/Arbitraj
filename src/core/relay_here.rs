//! Файл: core/relay_here.rs
//! Что делает: продажа «здесь» в режиме «Перегон» (решение 220, Тигран 08.11.2026; живой случай BRIC: перегон не
//! выгоднее → продано по рынку с убытком −30 %, −11 USDT, без всякого порога). Перед продажей здесь (перегон не
//! выгоднее, перегоны исчерпаны, площадка, где лежит монета, на паузе) убыток deal.loss_pct_if(выручка) сравнивается с
//! stuck.relay_max_loss_pct: не больше — продажа по рынку; больше — алерт человеку с кнопками (как «Порог убытка»),
//! сделка ждёт решения. Настройка не задана — функция не работает: алерт с причиной «не задана настройка» (правило
//! проекта: стартовых значений нет). Нет котировки — алерт с причиной (убыток неизвестен, вслепую не продаём).
//! Котировка — кошельком сделки (trade_ops::deal_value, решение 220 часть А).
//! Кто вызывает: core/relay.rs.
//! От чего зависит: trade_ops.rs, stuck.rs (sold_by_market), manual.rs (raise_alert), deal.rs.
//! Настройки: stuck.relay_max_loss_pct.

use rust_decimal::Decimal;

use crate::notify::human as h;
use crate::settings::read::At;

use super::deal::Deal;
use super::engine::Core;

/// Имя настройки порога убытка продажи здесь в режиме «Перегон».
pub const SETTING: &str = "stuck.relay_max_loss_pct";

/// Что делать с продажей здесь.
#[derive(Debug, Clone, PartialEq)]
pub enum Here {
    /// Продать по рынку — текст причины (для хронологии и уведомления stuck.market_sold).
    Market(String),
    /// Алерт человеку с кнопками: убыток (None — неизвестен) и причина.
    Alert { loss: Option<Decimal>, why: String },
}

/// Решение по продаже здесь (без сети — чистая функция). Принимает: сделку (для убытка), почему продаём здесь, порог
/// stuck.relay_max_loss_pct (Err — не задан, текст ошибки чтения), выручку при продаже сейчас (Err — нет котировки).
/// Возвращает: Here. Ошибок нет.
pub fn decide(
    deal: &Deal,
    why: &str,
    max: Result<Decimal, String>,
    value: Result<Decimal, String>,
) -> Here {
    let loss = value.as_ref().ok().map(|v| deal.loss_pct_if(*v));
    match (max, loss) {
        (Ok(max), Some(loss)) if loss <= max => Here::Market(format!(
            "{why}; убыток {} не больше порога {} ({SETTING})",
            h::pct(loss),
            h::pct(max)
        )),
        (Ok(max), Some(loss)) => Here::Alert {
            loss: Some(loss),
            why: format!(
                "{why}; убыток {} выше порога {} ({SETTING})",
                h::pct(loss),
                h::pct(max)
            ),
        },
        (Err(e), loss) => Here::Alert {
            loss,
            why: format!("{why}; продажа здесь не решена: не задана настройка {SETTING} ({e})"),
        },
        (Ok(_), None) => Here::Alert {
            loss: None,
            why: format!(
                "{why}; убыток продажи здесь неизвестен — нет котировки: {}",
                value.err().unwrap_or_default()
            ),
        },
    }
}

/// Продать здесь по рынку, если убыток не выше stuck.relay_max_loss_pct, иначе — алерт человеку. Принимает: ядро,
/// сделку, почему продаём здесь (для хронологии, журнала и Telegram). Ошибок нет: всё — в алерте или продаже.
pub async fn sell_here(core: &Core, deal: Deal, why: &str) {
    let value = super::trade_ops::deal_value(core, &deal)
        .await
        .map(|(v, _)| v);
    let max = core
        .inner
        .deps
        .settings
        .decimal(SETTING, At::default())
        .map_err(|e| e.message);
    match decide(&deal, why, max, value) {
        Here::Market(text) => super::stuck::sold_by_market(core, deal, &text).await,
        Here::Alert { loss, why } => super::manual::raise_alert(core, deal, loss, why, None).await,
    }
}
