//! Файл: core/subscribe.rs
//! Что делает: подписки площадок на рыночные данные по справочнику (шаг 2 ТЗ). Бирже — все её монеты
//! с сетью BSC и адресом контракта (стакан придёт, только если пара есть на споте — так ядро и узнаёт
//! «монета есть на споте»); DEX — только монеты, которыми можно торговать хотя бы на одной бирже (dex_tradable):
//! у биржи есть сеть BSC с адресом контракта и торгуемая спот-пара (биржа сообщает их событием SpotPairs из своего
//! справочника пар), плюс котируемая монета (USDT, адрес из справочника бирж) и промежуточные монеты (решение 52).
//! Пулы остальных монет не слушаются: каждое уведомление узла BSC платное, а по живому журналу 2026-10-24 половина
//! уведомлений о пулах приходилась на монеты без пары на BingX (BITLIGHT, IN, POP, ARIA, BTR, AIN, STAR…).
//! Порядок запуска: до первого SpotPairs список DEX пуст и подписки DEX нет; SpotPairs приходит из подписки биржи
//! и толкает пересборку — DEX подписывается один раз. Стаканы список DEX не меняют (ни первый стакан, ни
//! «недостоверно»), поэтому подписка DEX не дёргается при запуске и разрывах связи биржи.
//! Бирже также — стакан BNB (цена газа, convert.rs) и стаканы промежуточных монет (решение 52). Подписка меняется, только когда меняется список; выключенная площадка получает пустой список.
//! Кто вызывает: core/rebuild.rs после каждой пересборки.
//! От чего зависит: engine.rs, gateway (subscribe_market).
//! Настройки: нет своих.

use super::market_state::MarketState;
use super::registry::CoinEntry;
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
        // DEX — только монеты, которыми можно торговать хотя бы на одной бирже (dex_tradable). Флаги ввода и вывода
        // сюда не входят: биржа меняет их на время работ, а каждая смена списка — полная пересборка подписки DEX
        // с поиском пулов (тоже расход узла); направления по флагам отсекает справочник (registry.rs).
        VenueKind::Dex => {
            let kinds = rd(&core.inner.kinds);
            let market = rd(&core.inner.market);
            coins
                .iter()
                .filter(|c| dex_tradable(c, &kinds, &market))
                .map(|c| c.asset.clone())
                .collect()
        }
    };
    if kind == VenueKind::Dex && !out.is_empty() {
        // Котируемая монета (USDT) сама в справочник не входит, а адаптер DEX ищет пулы «монета — USDT»
        // и берёт адрес USDT из этого списка (venues/bsc_dex/tokens.rs). Адрес — из API бирж (registry::usdt_address).
        if let Some(q) = dex_quote(core, venue) {
            out.push(q);
        }
        // Промежуточные монеты (решение 52) — их пулы с USDT нужны маршруту «USDT → промежуточная → монета»;
        // тикер промежуточной адаптер берёт из этого же списка (venues/bsc_dex/pools/mids.rs).
        out.extend(intermediate_assets(core));
    }
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

/// Нужна ли монета подписке DEX: есть адрес контракта в BSC, и хотя бы у одной биржи у монеты есть сеть BSC
/// (данные сети с адресом — Presence::info) и торгуемая спот-пара (MarketEvent::SpotPairs этой биржи).
/// Монета, известная бирже только по адресу (сеть BSC есть, пары нет — например BITLIGHT), не нужна.
/// Принимает: монету справочника, виды площадок, состояние рынка. Возвращает: да/нет. Ошибок нет.
pub fn dex_tradable(c: &CoinEntry, kinds: &[(VenueId, VenueKind)], market: &MarketState) -> bool {
    c.asset.bsc_contract.is_some()
        && c.presence.iter().any(|p| {
            kinds
                .iter()
                .any(|(v, k)| v == &p.venue && *k == VenueKind::Cex)
                && p.info.is_some()
                && market.spot_pair(&p.venue, &c.asset.ticker)
        })
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

/// Котируемая монета площадки DEX: тикер — venue.quote_asset этой площадки, адрес — USDT в BSC из справочника бирж.
/// None — тикер не задан или адрес USDT ещё неизвестен (причина видна на экране «Монеты» и в журнале). Ошибок нет.
fn dex_quote(core: &Core, venue: &VenueId) -> Option<Asset> {
    let at = crate::settings::read::At {
        venue: Some(&venue.0),
        coin: None,
    };
    let ticker = core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .ok()?;
    let addr = rd(&core.inner.usdt).clone().ok()?;
    Some(Asset {
        ticker,
        bsc_contract: Some(addr),
    })
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
