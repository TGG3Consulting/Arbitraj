//! Файл: core/deal_words.rs
//! Что делает: решение 405 (t191, требование Тиграна 18.11: тревоги и тексты — чётко и конкретно, человеческими
//! словами) — общие куски текстов t191: какая сделка (ID, монета, откуда → куда, шаг, сколько монет и где), где лежат
//! деньги (площадка, у DEX — кошелёк), количество и сумма — теми же словами и округлением, что Telegram
//! (notify/human.rs). Тексты: remain_fact.rs (своих монет не видно, урезан остаток), manual_wait*.rs (ожидание ручной
//! кнопки), coin_counter.rs (встречная сделка), usdt_hold.rs / limits.rs (резерв USDT), transfers.rs (отмена вывода).
//! Кто вызывает: перечисленные файлы.
//! От чего зависит: deal.rs, deal_text.rs (шаг по-русски), trade_ops.rs (вид площадки), notify/human.rs (площадки,
//! количество, деньги), core/types.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::notify::human as hu;

use super::deal::Deal;
use super::engine::Core;

/// «BingX» / «DEX (кошелёк w1)»: кошелёк — только у площадки вида DEX (вид — справочник ядра, trade_ops::kind).
/// Принимает: ядро, площадку, кошелёк DEX (None — нет или неизвестен). Ошибок нет.
pub fn place(core: &Core, v: &VenueId, acc: Option<&DexAccountId>) -> String {
    match (super::trade_ops::kind(core, v), acc) {
        (Ok(VenueKind::Dex), Some(a)) => format!("{} (кошелёк {a})", hu::venue(&v.0)),
        _ => hu::venue(&v.0),
    }
}

/// «10 CAKE». Ошибок нет.
pub fn coins(q: Decimal, ticker: &str) -> String {
    format!("{} {ticker}", hu::qty(q))
}

/// «CAKE BingX → DEX». Ошибок нет.
pub fn route(d: &Direction) -> String {
    format!(
        "{} {}",
        d.asset.ticker,
        hu::route(&d.buy_on.0, &d.sell_on.0)
    )
}

/// «сделка D1 (CAKE BingX → DEX, шаг «перевод», 10 CAKE на BingX)». Принимает: ядро, сделку. Ошибок нет.
pub fn deal(core: &Core, d: &Deal) -> String {
    format!(
        "сделка {} ({}, шаг «{}», {} на {})",
        d.id,
        route(&d.direction),
        super::deal_text::step_name(d.step),
        coins(d.qty, &d.direction.asset.ticker),
        place(core, &d.at, d.dex_account.as_ref())
    )
}
