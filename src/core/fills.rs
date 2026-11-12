//! Файл: core/fills.rs
//! Что делает: перенос факта исполнения в учёт сделки: сколько монет у бота, сколько вложено и получено,
//! сколько потрачено на комиссии. Комиссия в монете уменьшает количество; в USDT или BNB — идёт в затраты
//! (BNB — по живой цене, convert.rs). Решение 219: комиссия ордера — список по монетам, каждая часть — отдельно; часть
//! со стоимостью в USDT из документации площадки (Fee.usd — баллы HTX, рынка нет) — в затраты как есть. Также затраты перевода (Transfer.fee). В хронологию сделки — кто исполнил своп
//! DEX (TradeResult.executor, решение 121 B7).
//! Решение 236, дополнение п. 8: газ обмена DEX (TradeResult.chain есть) — вид Gas, всегда в затраты, и у сделки с BNB
//! (решение 62, как в плане costs.rs); комиссия биржи в монете сделки — в количество, как раньше.
//! Кто вызывает: executor.rs, stuck.rs, manual.rs.
//! От чего зависит: deal.rs, convert.rs, settings, deal_book.rs (статьи затрат, решение 257).
//! Настройки: venue.quote_asset.

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, Side, TradeResult, Transfer, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::convert;
use super::deal::Deal;
use super::engine::{Core, rd};

/// Затрата в USDT по живым ценам. Ошибка — текст (нет курса): затрата остаётся неучтённой в итоге,
/// и это пишется в журнал (итог сделки помечается). Решение 236: им же — газ возврата USDT (deal_return_cost.rs).
pub(super) async fn usdt(
    core: &Core,
    venue: &VenueId,
    coin: &str,
    price: Decimal,
    c: &Cost,
) -> Result<Decimal, String> {
    let quote = core
        .inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&venue.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let cex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| v.clone())
        .collect();
    let rates = convert::rates(
        &core.inner.deps,
        &cex,
        vec![quote],
        coin,
        price,
        std::slice::from_ref(c),
    )
    .await?;
    convert::to_usdt(c, &rates)
}

/// Добавляет затрату: в монете — уменьшает количество; иначе — в spent (USDT). Газ — всегда в spent, даже у сделки
/// с BNB: он платится из резерва на газ, а не из монет сделки (решение 62, convert::in_coin) — так же, как в плане.
async fn charge(core: &Core, deal: &mut Deal, venue: &VenueId, price: Decimal, c: Cost) {
    if c.amount.is_zero() {
        return;
    }
    let coin = deal.direction.asset.ticker.clone();
    if convert::in_coin(&c, &coin) {
        deal.qty -= c.amount;
        return;
    }
    match usdt(core, venue, &coin, price, &c).await {
        // Решение 257: статья затрат — с видом затраты.
        Ok(u) => super::deal_book::spend(deal, c.kind, u),
        Err(e) => {
            let t = format!(
                "{}: затрата {} {} не переведена в USDT ({e}) — итог сделки без неё",
                deal.id, c.amount, c.asset
            );
            // Решение 164: в хронологии — коротко; подробности (все площадки и причины) — в журнале.
            deal.note(format!(
                "Комиссия {} {} не пересчитана в USDT — нет живой цены {}; итог сделки без неё",
                crate::notify::human::qty(c.amount),
                c.asset,
                c.asset
            ));
            core.inner
                .deps
                .log(Level::Warn, "core.ledger", Some(&venue.0), t, None)
                .await;
        }
    }
}

/// Что делает: вид затраты комиссии исполнения. Обмен в сети (TradeResult.chain есть — DEX) — газ: адаптеры DEX кладут
/// в fees только газ транзакции обмена (venues/bsc_dex/exec/result.rs, venues/kyberswap/trade_fact.rs — Fee::single(газ,
/// BNB)), он платится из BNB кошелька, а не из монет сделки — у сделки с BNB тоже (решение 62; так считает план —
/// CostKind::Gas, costs.rs). Биржа (chain нет) — торговая комиссия: в монете сделки уменьшает количество.
/// Решение 236, дополнение п. 8 (техлид 09.11 по поручению Тиграна): раньше газ шёл видом TradeFee — у сделки с BNB
/// convert::in_coin принимал его за комиссию в монете: количество уменьшалось, в spent газа не было (продажа BNB на DEX —
/// количество уходило в −газ, газ пропадал из итога). Принимает: итог. Возвращает: вид. Ошибок нет.
fn fee_kind(r: &TradeResult) -> CostKind {
    if r.chain.is_some() {
        CostKind::Gas
    } else {
        CostKind::TradeFee
    }
}

/// Факт покупки или продажи → учёт сделки. Ошибок нет.
pub async fn trade(core: &Core, deal: &mut Deal, r: &TradeResult) {
    let v = r.request.venue.clone();
    match r.request.side {
        Side::Buy => {
            deal.qty += r.filled_qty;
            deal.invested += r.filled_qty * r.avg_price;
        }
        Side::Sell => {
            deal.qty -= r.filled_qty;
            deal.proceeds += r.filled_qty * r.avg_price;
        }
    }
    // Решение 219: комиссия — по каждой монете отдельно (монета сделки уменьшает количество, остальное — в spent).
    // Газ обмена DEX — всегда в spent, и у сделки с BNB (fee_kind, решение 236, дополнение п. 8).
    for f in &r.fees {
        // Стоимость в USDT задана документацией площадки (Fee.usd: баллы HTX, рынка нет) — в затраты как есть.
        if let Some(u) = f.usd {
            super::deal_book::spend(deal, fee_kind(r), u);
            continue;
        }
        let fee = Cost::new(fee_kind(r), f.amount, f.asset.clone(), false);
        charge(core, deal, &v, r.avg_price, fee).await;
    }
    // Решение 121 B7: кто исполнил своп DEX (свой сборщик: прямой / умный / бандл или KyberSwap) — в хронологию сделки.
    let by = r
        .executor
        .as_ref()
        .map(|e| format!("; исполнил: {e}"))
        .unwrap_or_default();
    deal.note(format!(
        "{:?} на {}: {} по {} (комиссия {}){by}",
        r.request.side,
        v.0,
        r.filled_qty,
        r.avg_price,
        crate::core::types::Fee::text(&r.fees)
    ));
}

/// Факт перевода → учёт: монеты теперь на получателе в количестве «сколько дошло»; затраты вывода
/// не в монете (газ кошелька, у BNB тоже — из резерва) — в spent (в монете они уже в разнице «отправлено − дошло»).
/// Принимает: сделку, вывод (с комиссией), зачисление, получателя, цену монеты.
pub async fn transfer(
    core: &Core,
    deal: &mut Deal,
    sent: &Transfer,
    t: &Transfer,
    to: &VenueId,
    price: Decimal,
) {
    deal.qty = t.amount;
    deal.at = to.clone();
    if let Some(c) = sent.fee.clone()
        && !convert::in_coin(&c, &deal.direction.asset.ticker)
    {
        charge(core, deal, &sent.venue, price, c).await;
    }
    deal.transfers.push(sent.clone());
    deal.transfers.push(t.clone());
    deal.note(format!(
        "Перевод зачислен на {}: {} {}",
        to.0, t.amount, t.asset.ticker
    ));
}
