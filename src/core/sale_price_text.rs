//! Формирует ответ Telegram с сопоставимыми суммами на площадке первоначальной покупки.
use super::{deal::Deal, engine::Core, manual_price::Comparison, sale_price::SalePriceReply};
use crate::core::types::VenueId;
use crate::notify::human as hu;
use rust_decimal::Decimal;

fn venue(core: &Core, id: &VenueId) -> String {
    if super::daily_venue::is_dex(core, &id.0) {
        "DEX".into()
    } else {
        hu::venue(&id.0)
    }
}

pub(super) fn render(core: &Core, deal: &Deal, qty: Decimal, rates: &Comparison) -> SalePriceReply {
    let best = rates.better_target();
    let here = rates.here.as_ref().is_ok_and(|v| *v > Decimal::ZERO);
    let at_origin = deal.at == deal.direction.buy_on;
    let mut lines = vec![
        format!(
            "Запрос цены · {} · сделка {}",
            deal.direction.asset.ticker, deal.id
        ),
        format!(
            "Остаток: {} {}. Итог в USDT на площадке покупки ({}), с учётом комиссий:",
            qty.normalize(),
            deal.direction.asset.ticker,
            venue(core, &deal.direction.buy_on)
        ),
    ];
    if !deal.sells.is_empty() {
        lines.push("Итог включает выручку уже исполненных частей сделки.".into());
    }
    match &rates.here {
        Ok(v) => lines.push(format!(
            "На текущем месте ({}): {}. {}",
            venue(core, &deal.at),
            if v.is_sign_negative() {
                hu::signed_usd(*v)
            } else {
                hu::usd(*v)
            },
            super::sale_preview::pnl_text(deal, *v, rates.prior_cash)
        )),
        Err(e) => lines.push(format!(
            "На текущем месте ({}): расчёт недоступен — {e}",
            venue(core, &deal.at)
        )),
    }
    let best_venue = best.map(|(to, _)| venue(core, to));
    if let Some((to, amount)) = best {
        lines.push(format!(
            "Лучшая площадка: {} · {}. {}",
            venue(core, to),
            hu::usd(amount),
            super::sale_preview::pnl_text(deal, amount, rates.prior_cash)
        ));
        if let Some((delta, pct)) = rates.difference(amount) {
            let percent =
                pct.map_or_else(|| "процент не определён".into(), |p| format!("+{p:.2}%"));
            lines.push(format!(
                "Больше, чем здесь: {} ({percent})",
                hu::signed_usd(delta)
            ));
        } else {
            lines.push("Разницу посчитать нельзя: нет цены на текущем месте.".into());
        }
    } else if here {
        lines.push(format!(
            "Лучший доступный вариант: {}. Прирост к продаже здесь: 0 USDT (0%).",
            venue(core, &deal.at)
        ));
    } else {
        lines.push("Доступной продажи с возвратом USDT не найдено.".into());
    }
    if best.is_none() && !rates.targets.iter().any(|(_, r)| r.is_ok()) {
        if let Some((to, Err(reason))) = rates.targets.first() {
            lines.push(format!("{}: {reason}", venue(core, to)));
        } else if let Some(reason) = rates.skipped.first() {
            lines.push(reason.clone());
        }
    }
    SalePriceReply {
        text: lines.join("\n"),
        best_venue,
        here: None,
        away: None,
        at_origin,
    }
}
