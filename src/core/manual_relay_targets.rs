//! Файл: core/manual_relay_targets.rs
//! Что делает: решение 235 — цели перегона кнопки «Продать и перегнать» (manual_relay.rs) по настройкам и справочнику
//! монет: у монеты там рынок, приём по BSC и продажа через API; venue.enabled = да; площадка не на паузе; биржа ↔
//! биржа — только если venue.cex_cex не «нет» у обеих сторон: у биржи, где лежит монета, и у биржи-цели (не задана —
//! да; «у одной из сторон», как направления решения 214); на DEX и «продать здесь» — без этого условия; DEX — кошельком по
//! маппингу (dex_account::for_relay, решение 185), кошелёк не на паузе (решение 230). Решение 248: условие биржа ↔ биржа —
//! cex_link::relay_block: биржа-цель не берётся, если venue.cex_cex = нет у неё или у любой биржи, через которую монета
//! сделки уже прошла (покупка, переводы, где лежит сейчас — в том числе когда монета сейчас на кошельке DEX), или если
//! она в списке venue.cex_cex_deny биржи покупки. С площадки, где лежит монета,
//! перегона нет, если вывод с неё по BSC закрыт, она или её кошелёк на паузе, или она уже отказала в выводе в этой
//! сделке (решения 220, 227, 230). Каждая отсеянная площадка — с причиной (в хронологию сделки).
//! Кто вызывает: core/manual_relay.rs.
//! От чего зависит: ticker_index.rs, registry_sync.rs (enabled), dex_account.rs, wallet_pause.rs, withdraw_refused.rs,
//! trade_ops.rs (вид площадки), engine.rs (справочник монет, пауза площадок).
//! Настройки: venue.enabled; venue.cex_cex, venue.cex_cex_deny (по площадке, через cex_link.rs).

use crate::core::types::VenueId;

use super::deal::Deal;
use super::engine::{Core, lk, rd};

/// Цели перегона по настройкам и справочнику. Принимает: ядро, сделку. Возвращает: (цели; причины, почему площадки
/// не цели или перегона нет вовсе — для хронологии). Ошибок нет.
pub fn targets(core: &Core, deal: &Deal) -> (Vec<VenueId>, Vec<String>) {
    let from = &deal.at;
    let mut skip = vec![];
    if let Some(p) = lk(&core.inner.risk).venue_paused.get(from).cloned() {
        skip.push(format!("{} на паузе ({p}) — перегона с неё нет", from.0));
    }
    if let Some(p) = super::wallet_pause::paused_at(core, deal) {
        skip.push(format!(
            "{} на паузе ({}) — перегона с него нет",
            p.who(),
            p.why
        ));
    }
    if super::withdraw_refused::refused(deal, from) {
        skip.push(format!(
            "вывод с {} в этой сделке уже отклонён — перегона с неё нет",
            from.0
        ));
    }
    let coins = rd(&core.inner.coins);
    let Some(e) = super::ticker_index::find(core, &coins, &deal.direction.asset) else {
        skip.push("монеты нет в справочнике — перегона нет".into());
        return (vec![], skip);
    };
    if !e.at(from).is_some_and(|p| p.can_send) {
        skip.push(format!("вывод с {} по BSC закрыт — перегона нет", from.0));
    }
    if !skip.is_empty() {
        return (vec![], skip);
    }
    let paused = lk(&core.inner.risk).venue_paused.clone();
    let mut out = vec![];
    for p in e
        .presence
        .iter()
        .filter(|p| p.excluded.is_empty() && &p.venue != from)
    {
        let to = &p.venue;
        // Решение 248: правила биржа ↔ биржа относительно пути монеты сделки (cex_link.rs).
        let link = super::cex_link::relay_block(core, deal, to);
        let why = if !p.can_receive || !p.can_sell {
            Some("приём по BSC или продажа через API закрыты".to_string())
        } else if !super::registry_sync::enabled(core, to) {
            Some("площадка выключена (venue.enabled)".into())
        } else if let Some(x) = paused.get(to) {
            Some(format!("на паузе ({x})"))
        } else if let Some(w) = link {
            Some(w)
        } else {
            match super::dex_account::for_relay(core, deal, to) {
                Err(e) => Some(e),
                Ok(acc) => super::wallet_pause::paused(core, acc.as_ref())
                    .map(|w| format!("{} на паузе ({})", w.who(), w.why)),
            }
        };
        match why {
            Some(w) => skip.push(format!("{}: {w}", to.0)),
            None => out.push(to.clone()),
        }
    }
    (out, skip)
}
