//! Файл: core/rebuild_blacklist.rs
//! Что делает: чёрный список при пересборке справочника (core/rebuild.rs): ручная отметка и проверки налогов со всех
//! DEX, где у монеты есть пул с USDT; заполняет block / problem / wait у монет. Нет проверок, потому что пулов монеты
//! сейчас нет, потому что сняты до перечитывания (Invalidated — обрыв ленты квитанций, решение 71; отметка inner.held),
//! — вердикт не меняется. Пула нет по справочнику (NoUsdtPool без отметки) — вердикт снимается, как раньше.
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: blacklist.rs, registry.rs, market_state.rs, engine.rs, settings.
//! Настройки: coin.*, tax.*, hooks.* — через blacklist.rs.

use chrono::Utc;

use super::blacklist::{self, BlockChange, TaxLimits};
use super::engine::{Core, rd, wr};
use super::rebuild::has_pool;
use super::registry::CoinEntry;

/// Чёрный список по справочнику: ручная отметка, проверки налогов со всех DEX, где у монеты есть
/// пул с USDT. Заполняет block / problem у монет. Возвращает: изменения для уведомлений.
pub fn apply_blacklist(core: &Core, coins: &mut [CoinEntry]) -> Vec<BlockChange> {
    let s = &core.inner.deps.settings;
    let limits = TaxLimits::read(s);
    let market = rd(&core.inner.market);
    let mut bl = wr(&core.inner.blacklist);
    let now = Utc::now();
    let mut changes = vec![];
    for c in coins.iter_mut() {
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
        let manual = blacklist::manual(s, &c.asset.ticker);
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
