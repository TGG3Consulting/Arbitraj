//! Файл: core/usdt_return_split.rs
//! Что делает: решение 227 — части возврата выручки USDT закрытой сделки, продававшей на НЕСКОЛЬКИХ площадках (живой
//! случай 20261108104958308JCT: часть на Bitget, остаток перегнан на DEX и продан там). Часть = площадка выручки
//! (кроме площадки покупки), сумма = выручка продаж там минус их комиссия в котируемой монете этой площадки
//! (usdt_return_parts::split). Исключённый путь («Продано вручную», решения 100 и 224): в sells только продажи бота —
//! части = выручка бота по площадкам; выручку человека бот не перегоняет (запись в хронологии), а если частей бота нет
//! — страховка «USDT не вернулись» на выручку человека, как раньше. Частей нет («Продать и перегнать», всё продано на
//! площадке покупки) — журнал «перегон не нужен».
//! Кто вызывает: core/usdt_return_deal.rs (prepare).
//! От чего зависит: usdt_return_parts.rs, usdt_return_deal.rs (not_returned, no_need, quote_of), deal.rs.
//! Настройки: venue.quote_asset (по площадке части, через usdt_return_deal::quote_of).

use rust_decimal::Decimal;

use crate::core::types::VenueId;

use super::deal::Deal;
use super::engine::Core;
use super::usdt_return_parts as parts;

/// Части возврата сделки с продажами на нескольких площадках. Принимает: ядро, сделку (страховка и записи хронологии
/// пишутся в неё). Возвращает: [(откуда, сумма)] — пусто, если возврата нет. Ошибок нет.
pub async fn parts(core: &Core, deal: &mut Deal) -> Vec<(VenueId, Decimal)> {
    let out = parts::split(deal, |v| super::usdt_return_deal::quote_of(core, v));
    if let Some(why) = deal.usdt_return_skip.clone() {
        // В sells — только продажи бота; всё сверх их выручки в deal.proceeds — ввод человека («Продано вручную»).
        let bot: Decimal = parts::sell_venues(deal)
            .iter()
            .map(|v| parts::sold_at(deal, v))
            .sum();
        let human = deal.proceeds - bot;
        if out.is_empty() {
            if human > Decimal::ZERO {
                let at = deal.at.clone();
                super::usdt_return_deal::not_returned(core, deal, human, &at, &why).await;
            }
            return out;
        }
        if human > Decimal::ZERO {
            let sum: Decimal = out.iter().map(|p| p.1).sum();
            deal.note(format!(
                "Сделка {}: выручку человека {human} USDT бот не перегоняет ({why}); перегоняется выручка продаж бота {sum} USDT по частям (решения 224, 227)",
                deal.id
            ));
        }
        return out;
    }
    if out.is_empty() && deal.return_usdt {
        super::usdt_return_deal::no_need(core, deal).await;
    }
    out
}
