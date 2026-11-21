//! Файл: core/usdt_hold_view.rs
//! Что делает: решение 405 (t191, ревью 4, Н2) — резерв возвратов USDT на экране «Ручное управление»: по каждой
//! площадке (у DEX — кошельку) строки резерва (usdt_hold.rs held — те же суммы, что вычитаются из свободного у покупки,
//! ручного перевода и чужих перегонов): сделка, что ждёт (общий возврат группы, возврат идёт, ждёт повтора, не
//! вернулись — ждут человека), сумма. «Не вернулись» резервируется до кнопки человека (ответ Тиграна 18.11) — старая
//! запись уменьшает покупки, пока её не разберут; у такой строки — ключ части (usdt_return_parts::key: ID сделки у
//! первой части, «ID@площадка» у остальных), экран рисует кнопки записи «u.<ключ>» («Перегнать», «Перегнано
//! вручную»), те же, что в разделе «USDT не вернулись».
//! Кто вызывает: manual_return.rs (Core::manual — reserves).
//! От чего зависит: usdt_hold.rs, unsettled.rs, usdt_return_parts.rs, usdt_return_state.rs, engine.rs, trade_ops.rs.
//! Настройки: нет.

use std::collections::BTreeSet;

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Строка резерва для экрана.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// Площадка, где лежат USDT.
    pub venue: String,
    /// Кошелёк DEX (у биржи — нет).
    pub wallet: Option<String>,
    /// Сделка.
    pub deal: String,
    /// Что ждёт.
    pub what: String,
    /// Сумма в резерве, USDT.
    pub amount: Decimal,
    /// Ключ части «не вернулись» для кнопок записи «u.<ключ>»; у остальных строк — нет.
    pub key: Option<String>,
}

/// Площадки (и кошелёк DEX), где у сделок набора unsettled что-то в резерве. Ошибок нет.
fn places(core: &Core) -> BTreeSet<(String, Option<String>)> {
    let ids = super::unsettled::ids(core);
    let deals = lk(&core.inner.deals);
    let mut out = BTreeSet::new();
    for d in ids.iter().filter_map(|id| deals.get(id)) {
        let mut venues: Vec<&VenueId> = vec![];
        if let Some((v, _)) = d.group.as_ref().and_then(|g| g.wait.as_ref()) {
            venues.push(v);
        }
        for p in super::usdt_return_parts::all(d) {
            if matches!(
                p,
                UsdtReturn::Pending { .. }
                    | UsdtReturn::Retrying { .. }
                    | UsdtReturn::NotReturned { .. }
            ) {
                venues.push(p.venue());
            }
        }
        for v in venues {
            out.insert((v.0.clone(), d.dex_account.as_ref().map(|a| a.0.clone())));
        }
    }
    out
}

/// Ключ части «не вернулись» сделки на площадке (как usdt_return_parts::key); None — там не «не вернулись». Ошибок нет.
fn not_returned_key(core: &Core, deal: &str, v: &VenueId) -> Option<String> {
    let deals = lk(&core.inner.deals);
    let d = deals.get(deal)?;
    if let Some(UsdtReturn::NotReturned { at, .. }) = &d.usdt_return
        && at == v
    {
        return Some(d.id.clone());
    }
    d.usdt_return_parts
        .iter()
        .any(|p| matches!(p, UsdtReturn::NotReturned { at, .. } if at == v))
        .then(|| format!("{}@{}", d.id, v.0))
}

/// Что делает: резерв возвратов USDT по площадкам и кошелькам (GET /api/manual — reserves). Ошибок нет.
pub fn rows(core: &Core) -> Vec<Row> {
    let mut out = vec![];
    let mut seen_cex = BTreeSet::new();
    // Кошелёк неизвестен у какой-то сделки — её строка попадёт и в другие кошельки этой площадки: одна строка на
    // (площадка, сделка, что ждёт).
    let mut seen = BTreeSet::new();
    for (venue, wallet) in places(core) {
        let v = VenueId(venue.clone());
        let cex = super::trade_ops::kind(core, &v) == Ok(VenueKind::Cex);
        // Биржа — один счёт: строки по площадке один раз, без кошелька.
        if cex && !seen_cex.insert(venue.clone()) {
            continue;
        }
        let wallet = if cex { None } else { wallet };
        let acc = wallet.clone().map(DexAccountId);
        let h = super::usdt_hold::held(core, &v, acc.as_ref());
        for (deal, what, amount) in h.lines {
            if !seen.insert((venue.clone(), deal.clone(), what.clone())) {
                continue;
            }
            let key = not_returned_key(core, &deal, &v);
            out.push(Row {
                venue: venue.clone(),
                wallet: wallet.clone(),
                deal,
                what,
                amount,
                key,
            });
        }
    }
    out
}
