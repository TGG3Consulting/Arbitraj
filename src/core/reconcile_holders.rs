//! Файл: core/reconcile_holders.rs
//! Что делает: кто сверяется в круге сверки балансов (решение 132): биржа — сама площадка; DEX — каждый кошелёк
//! отдельно (свои балансы, учёт сделок кошелька, «мало USDT/BNB»). Кошельков нет — DEX не сверяется (торговли с DEX
//! без кошелька нет; без счёта адаптер балансы не отдаёт). Подпись — «площадка «имя кошелька»».
//! Кто вызывает: core/reconcile.rs.
//! От чего зависит: engine.rs, settings/accounts/ (список кошельков).
//! Настройки: нет.

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::Core;

/// Строка сверки: площадка, её вид, счёт DEX (None — биржа), подпись.
pub type Holder = (VenueId, VenueKind, Option<DexAccountId>, String);

/// Список строк сверки. Принимает: ядро, включённые площадки с видами. Возвращает: строки. Ошибок нет.
pub fn list(core: &Core, venues: Vec<(VenueId, VenueKind)>) -> Vec<Holder> {
    let mut out = Vec::new();
    for (v, kind) in venues {
        if kind != VenueKind::Dex {
            let label = v.0.clone();
            out.push((v, kind, None, label));
            continue;
        }
        for w in core.inner.deps.settings.accounts().wallets() {
            let label = format!("{} «{}»", v.0, w.name);
            out.push((v.clone(), kind, Some(DexAccountId(w.id)), label));
        }
    }
    out
}
