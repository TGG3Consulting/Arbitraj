//! Файл: core/rebuild_blacklist.rs
//! Что делает: чёрный список при пересборке справочника (core/rebuild.rs): ручная отметка и проверки налогов со всех
//! DEX, где у монеты есть пул с USDT; заполняет block / problem / wait у монет. Нет проверок, потому что пулов монеты
//! сейчас нет, потому что сняты до перечитывания (Invalidated — обрыв ленты квитанций, решение 71; отметка inner.held),
//! — вердикт не меняется. Пула нет по справочнику (NoUsdtPool без отметки) — вердикт снимается, как раньше.
//! Биржа ↔ биржа (решение 118, Я-3): налог перевода не измерен ни одной DEX и cex_cex.untaxed_ok не «да» — поле
//! CoinEntry.cex_cex с причиной, таких направлений нет.
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: blacklist.rs, registry.rs, market_state.rs, engine.rs, settings.
//! Решение 214: биржи с venue.cex_cex = нет — поле CoinEntry.cex_cex_off, направлений биржа ↔ биржа с ними нет.
//! Настройки: coin.*, tax.*, hooks.* — через blacklist.rs; cex_cex.untaxed_ok — через transfer_tax.rs; venue.cex_cex.

use chrono::Utc;

use super::blacklist::{self, BlockChange, TaxLimits};
use super::engine::{Core, rd, wr};
use super::market_state::MarketState;
use super::rebuild::has_pool;
use super::registry::CoinEntry;

/// Решение 214: разрешено ли бирже торговать с другими биржами (venue.cex_cex). Не задана — да (Тигран, 07.11.2026).
/// Принимает: настройки, площадку. Возвращает: true — разрешено. Ошибок нет.
fn cex_cex_allowed(s: &crate::settings::store::Store, venue: &str) -> bool {
    s.flag(
        "venue.cex_cex",
        crate::settings::read::At {
            venue: Some(venue),
            coin: None,
        },
    )
    .unwrap_or(true)
}

/// Чёрный список по справочнику: ручная отметка, проверки налогов со всех DEX, где у монеты есть
/// пул с USDT. Заполняет block / problem у монет. Возвращает: изменения для уведомлений.
pub fn apply_blacklist(
    core: &Core,
    market: &MarketState,
    coins: &mut [CoinEntry],
) -> Vec<BlockChange> {
    let s = &core.inner.deps.settings;
    let limits = TaxLimits::read(s);
    let mut bl = wr(&core.inner.blacklist);
    let now = Utc::now();
    let kinds = rd(&core.inner.kinds).clone();
    let untaxed_ok = super::transfer_tax::untaxed_ok(s);
    let mut changes = vec![];
    for c in coins.iter_mut() {
        // Решение 118 (Я-3): биржа ↔ биржа без измеренного налога перевода — только при cex_cex.untaxed_ok = да.
        let cexes = c.presence.iter().filter(|p| p.info.is_some()).count();
        let measured = c
            .asset
            .bsc_contract
            .and_then(|t| super::transfer_tax::measured(market, &kinds, t));
        c.cex_cex = (cexes >= 2 && measured.is_none() && !untaxed_ok)
            .then(|| super::transfer_tax::UNMEASURED.to_string());
        // Решение 214: биржи с venue.cex_cex = нет (не задана — да) — без направлений биржа ↔ биржа.
        c.cex_cex_off = c
            .presence
            .iter()
            .filter(|p| p.info.is_some() && !cex_cex_allowed(s, &p.venue.0))
            .map(|p| p.venue.clone())
            .collect();
        let checks: Vec<_> = match c.asset.bsc_contract {
            Some(token) => c
                .presence
                .iter()
                .filter(|p| p.info.is_none() && has_pool(&p.excluded))
                .map(|p| market.coin_check(&p.venue, token))
                .collect(),
            None => vec![],
        };
        // Причина ожидания проверки налогов (CoinCheck.wait) — для «Живого экрана» и «Монет».
        c.wait = checks
            .iter()
            .flatten()
            .filter(|k| k.taxes == crate::core::types::CheckState::NotChecked)
            .find_map(|k| k.wait.clone());
        let tickers: Vec<&str> = c.presence.iter().map(|p| p.ticker.as_str()).collect();
        let manual = blacklist::manual(s, &c.asset, &tickers);
        // Проверок нет, потому что пулов монеты сейчас нет (Invalidated: пулы сняты до перечитывания — обрыв ленты
        // квитанций, решение 71): данных нет — вердикт чёрного списка не меняется (не «вышла» и не «вошла» заново).
        if keep_verdict(
            manual.is_none(),
            checks.is_empty(),
            bl.get(&c.asset).is_some(),
            held(core, c),
        ) {
            c.block = bl.get(&c.asset).cloned();
            continue;
        }
        match blacklist::reason(manual, &checks, &limits) {
            Ok(r) => {
                changes.push(bl.set(&c.asset, r, now));
                c.block = bl.get(&c.asset).cloned();
            }
            Err(e) => c.problem = Some(e),
        }
    }
    changes
}

/// Пулы монеты на DEX сняты до перечитывания (Invalidated, inner.held), а не исчезли из справочника. Ошибок нет.
pub fn held(core: &Core, c: &CoinEntry) -> bool {
    let Some(token) = c.asset.bsc_contract else {
        return false;
    };
    let held = super::engine::lk(&core.inner.held);
    c.presence
        .iter()
        .any(|p| held.contains(&(p.venue.clone(), token)))
}

/// Держать ли решение по чёрному списку без пересчёта: ручной отметки нет, проверок нет, монета в списке и её пулы
/// сняты до перечитывания (Invalidated). Пула нет по справочнику (не снят, а нет) — пересчёт (монета выходит из
/// списка). Принимает: признаки. Ошибок нет.
pub fn keep_verdict(no_manual: bool, no_checks: bool, listed: bool, held: bool) -> bool {
    no_manual && no_checks && listed && held
}
