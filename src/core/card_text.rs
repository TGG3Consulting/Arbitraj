//! Файл: core/card_text.rs
//! Что делает: текст карточки сигнала (решение 74) для журнала и Telegram (deal.candidate, episode.opened): монета и
//! контракт, где купить и продать, количество и средние цены, стакан и пулы с ликвидностью в полосе, прибыль на бюджете,
//! максимальный объём и прибыль без бюджета с причиной края поиска, ссылки (или «шаблон ссылки не задан»).
//! Кто вызывает: core/candidates.rs, core/episode_run.rs.
//! От чего зависит: card.rs, card_book.rs, card_links.rs, size_max.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::Side;

use super::card::Card;

/// Число для текста: 6 знаков после точки без хвостовых нулей. Ошибок нет.
fn n(v: Decimal) -> String {
    v.round_dp(6).normalize().to_string()
}

/// Что делает: текст карточки. Принимает: карточку. Возвращает: многострочный текст. Ошибок нет.
pub fn text(c: &Card) -> String {
    let mut l = Vec::new();
    let mark = if c.dry { "[СУХОЙ] " } else { "" };
    l.push(format!(
        "{mark}Сигнал: {} {}",
        c.ticker,
        c.contract.as_deref().unwrap_or("(контракт неизвестен)")
    ));
    l.push(format!("Купить на {} → продать на {}", c.buy_on, c.sell_on));
    l.push(format!(
        "Купить {} {} по {} (средняя), продать {} по {}",
        n(c.qty),
        c.ticker,
        n(c.buy_avg),
        n(c.sell_qty),
        n(c.sell_avg)
    ));
    for b in &c.books {
        let side = if b.side == Side::Buy { "asks" } else { "bids" };
        let body = match (&b.error, b.usd_in_band, b.band_pct) {
            (Some(e), _, _) => e.clone(),
            (None, Some(usd), Some(band)) => format!(
                "лучшая {}, в полосе ±{}% — {} USDT ({} уровней{})",
                b.best.map(n).unwrap_or_default(),
                n(band),
                n(usd),
                b.levels,
                if b.beyond {
                    "; полоса шире загруженных уровней venue.depth_levels"
                } else {
                    ""
                }
            ),
            _ => "нет данных".into(),
        };
        l.push(format!("Стакан {} ({side}): {body}", b.venue));
    }
    for p in &c.pools {
        let side = if p.side == Side::Buy {
            "покупки"
        } else {
            "продажи"
        };
        let body = match (p.depth_usd, &p.error) {
            (Some(d), _) => format!(
                "глубина {side} {} USDT в полосе ±{}%{}{}",
                n(d),
                p.band_pct.map(n).unwrap_or_default(),
                if p.deep { "" } else { " (мелкий)" },
                if p.full {
                    ""
                } else {
                    ", полоса прочитана не целиком"
                }
            ),
            (None, Some(e)) => e.clone(),
            (None, None) => "глубина не посчитана".into(),
        };
        l.push(format!(
            "Пул {} {:?} {}: {body}, блок {}",
            p.venue, p.kind, p.pool, p.block
        ));
    }
    let budget = c.budget.map(n).unwrap_or_else(|| "—".into());
    l.push(format!(
        "Прибыль на бюджете {budget} USDT: {} USDT ({}%)",
        n(c.profit_usd),
        c.profit_pct.round_dp(4).normalize()
    ));
    l.push(match &c.max {
        Ok(m) => format!(
            "Без бюджета: макс. объём {} (прибыль {}), макс. прибыль {} USDT на {}; край поиска — {}",
            m.max_qty.map(n).unwrap_or_else(|| "—".into()),
            m.max_profit.map(n).unwrap_or_else(|| "—".into()),
            n(m.opt_profit),
            n(m.opt_qty),
            m.limit
        ),
        Err(e) => format!("Без бюджета: не посчитано — {e}"),
    });
    for k in &c.links {
        let what = if k.kind == "pair" {
            "Пара"
        } else {
            "Пул"
        };
        let target = k.url.clone().or_else(|| k.why.clone()).unwrap_or_default();
        l.push(format!("{what} {} {}: {target}", k.venue, k.label));
    }
    l.join("\n")
}
