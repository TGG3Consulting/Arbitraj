//! Файл: core/subscribe.rs
//! Что делает: подписки площадок на рыночные данные по справочнику (шаг 2 ТЗ). Бирже — все её монеты
//! с сетью BSC и адресом контракта (стакан придёт, только если пара есть на споте — так ядро и узнаёт
//! «монета есть на споте»); DEX — все монеты с адресом контракта у бирж (адаптер ищет их пулы с USDT).
//! Бирже также — стакан BNB (цена газа, convert.rs) и стаканы промежуточных монет (решение 52). Подписка меняется, только когда меняется список; выключенная площадка получает пустой список.
//! Кто вызывает: core/rebuild.rs после каждой пересборки.
//! От чего зависит: engine.rs, gateway (subscribe_market).
//! Настройки: нет своих.

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::engine::{Core, rd};

/// Нужный список монет площадки по текущему справочнику. Список отсортирован (для сравнения).
fn wanted(core: &Core, venue: &VenueId, kind: VenueKind) -> Vec<Asset> {
    let coins = rd(&core.inner.coins);
    let mut out: Vec<Asset> = match kind {
        VenueKind::Cex => coins
            .iter()
            .filter(|c| {
                c.presence
                    .iter()
                    .any(|p| &p.venue == venue && p.info.is_some())
            })
            .map(|c| c.asset.clone())
            .collect(),
        VenueKind::Dex => coins
            .iter()
            .filter(|c| c.asset.bsc_contract.is_some())
            .map(|c| c.asset.clone())
            .collect(),
    };
    if kind == VenueKind::Cex && !out.is_empty() {
        // Цена газа в USDT — из живого стакана BNB биржи (решение 35, convert.rs).
        out.push(super::convert::gas_asset());
        // Живые цены промежуточных монет (решение 52) — тикер по адресу из справочника бирж.
        out.extend(intermediate_assets(core));
    }
    out.sort_by(|a, b| a.ticker.cmp(&b.ticker));
    out.dedup();
    out
}

/// Приводит подписки всех площадок к справочнику.
/// Принимает: ядро, включённые площадки. Ошибок нет — сбой подписки пишется в журнал,
/// и при следующей пересборке подписка повторяется.
pub async fn sync(core: &Core, enabled: &[(VenueId, VenueKind)]) {
    let all = rd(&core.inner.kinds).clone();
    let deps = &core.inner.deps;
    let mut subs = core.inner.subs.lock().await;
    for (venue, kind) in all {
        let want = if enabled.iter().any(|(v, _)| v == &venue) {
            wanted(core, &venue, kind)
        } else {
            vec![]
        };
        let have = subs.get(&venue).cloned().unwrap_or_default();
        if want == have {
            continue;
        }
        let res = match deps.gw.get(&venue).await {
            Ok(v) => {
                v.subscribe_market(want.clone(), core.inner.market_tx.clone())
                    .await
            }
            Err(e) => Err(e),
        };
        match res {
            Ok(()) => {
                deps.log(
                    Level::Info,
                    "core.subscribe",
                    Some(&venue.0),
                    format!("Подписка на рыночные данные: {} монет", want.len()),
                    None,
                )
                .await;
                subs.insert(venue, want);
            }
            Err(e) => {
                deps.log(
                    Level::Warn,
                    "core.subscribe",
                    Some(&venue.0),
                    format!("Подписка не удалась: {}", e.message),
                    None,
                )
                .await;
                subs.remove(&venue);
            }
        }
    }
}

/// Промежуточные монеты всех площадок DEX, которые биржи знают по адресу (тикер для стакана). Ошибок нет.
fn intermediate_assets(core: &Core) -> Vec<Asset> {
    let s = &core.inner.deps.settings;
    let nets = rd(&core.inner.networks);
    let mut out = vec![];
    for (v, _) in rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
    {
        if let super::intermediate::Intermediate::On(list) = super::intermediate::read(s, v) {
            for a in list {
                if let Some(t) = super::intermediate::ticker_of(&nets, a) {
                    out.push(Asset {
                        ticker: t,
                        bsc_contract: Some(a),
                    });
                }
            }
        }
    }
    out
}
