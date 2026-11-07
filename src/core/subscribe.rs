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
//! Бирже также — стакан BNB (цена газа, convert.rs), стаканы промежуточных монет (решение 52) и монет venue.price_assets
//! (решение 181: комиссия Gate в GT — пересчёт в USDT по живой цене). Подписка меняется, только когда меняется список; выключенная площадка получает пустой список,
//! а с идущими сделками — держит поток до их закрытия (решение 196, feed_hold.rs).
//! Кто вызывает: core/rebuild.rs после каждой пересборки; подписка площадки — отдельной задачей (sub_state.rs).
//! От чего зависит: engine.rs, gateway (subscribe_market).
//! Настройки: venue.price_assets [биржа] (необязательная).

use super::market_state::MarketState;
use super::registry::{CoinEntry, Presence};
use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::engine::{Core, lk, rd};
use super::sub_state::Note;

/// Нужный список монет площадки по текущему справочнику. Список отсортирован (для сравнения).
fn wanted(core: &Core, venue: &VenueId, kind: VenueKind) -> Vec<Asset> {
    let coins = rd(&core.inner.coins);
    let mut out: Vec<Asset> = match kind {
        VenueKind::Cex => coins
            .iter()
            // Решение 211: бирже — монета с ЕЁ тикером (пара и стакан площадки — по её имени), адрес тот же.
            .filter_map(|c| {
                c.presence
                    .iter()
                    .find(|p| &p.venue == venue && p.info.is_some())
                    .map(|p| Asset {
                        ticker: p.ticker.clone(),
                        bsc_contract: c.asset.bsc_contract,
                    })
            })
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
        out.extend(intermediate_assets(core, None));
    }
    if kind == VenueKind::Cex && !out.is_empty() {
        // Цена газа в USDT — из живого стакана BNB биржи (решение 35, convert.rs).
        out.push(super::convert::gas_asset());
        // Живые цены промежуточных монет (решение 52) — тикер ЭТОЙ биржи по адресу (решение 211).
        out.extend(intermediate_assets(core, Some(venue)));
        // Решение 181: монеты для пересчёта затрат в USDT (комиссия Gate в GT) — venue.price_assets биржи.
        out.extend(price_assets(core, venue));
    }
    // Решение 211: одна монета (адрес) — один раз; первой идёт монета справочника с тикером площадки.
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert(a.clone()));
    out.sort_by(|a, b| a.ticker.cmp(&b.ticker));
    if kind == VenueKind::Cex
        && let Some(n) = super::native::ticker(core)
    {
        // Нативная монета (решение 62): монета справочника и монета газа — один стакан, одна подписка.
        out.dedup_by(|a, b| a.ticker == n && b.ticker == n);
    }
    out
}

/// Решение 181: монеты из venue.price_assets биржи — только цена (стакан), без адреса контракта, как BNB газа.
/// Не задана — ничего (как раньше). Ошибок нет.
fn price_assets(core: &Core, venue: &VenueId) -> Vec<Asset> {
    let at = crate::settings::read::At {
        venue: Some(&venue.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .list("venue.price_assets", at)
        .unwrap_or_default()
        .into_iter()
        .map(|t| Asset {
            ticker: t.trim().to_uppercase(),
            bsc_contract: None,
        })
        .filter(|a| !a.ticker.is_empty())
        .collect()
}

/// Нужна ли монета подписке DEX: есть адрес контракта в BSC, и хотя бы у одной биржи у монеты есть сеть BSC
/// (данные сети с адресом — Presence::info) и торгуемая спот-пара (MarketEvent::SpotPairs этой биржи).
/// Монета, известная бирже только по адресу (сеть BSC есть, пары нет — например BITLIGHT), не нужна.
/// Принимает: монету справочника, виды площадок, состояние рынка. Возвращает: да/нет. Ошибок нет.
pub fn dex_tradable(c: &CoinEntry, kinds: &[(VenueId, VenueKind)], market: &MarketState) -> bool {
    c.asset.bsc_contract.is_some() && cex_pair(&c.presence, kinds, market)
}

/// Есть ли у монеты на какой-либо бирже сеть BSC (данные сети — Presence::info) и торгуемая спот-пара — то же
/// условие подписки DEX (dex_tradable), и по нему же справочник ставит монете DEX-присутствие (registry_build.rs).
/// Решение 211: пара ищется по тикеру ЭТОЙ биржи (Presence::ticker).
/// Принимает: присутствия монеты, виды площадок, состояние рынка. Возвращает: да/нет. Ошибок нет.
pub fn cex_pair(
    presence: &[Presence],
    kinds: &[(VenueId, VenueKind)],
    market: &MarketState,
) -> bool {
    presence.iter().any(|p| {
        kinds
            .iter()
            .any(|(v, k)| v == &p.venue && *k == VenueKind::Cex)
            && p.info.is_some()
            && market.spot_pair(&p.venue, &p.ticker)
    })
}

/// Приводит подписки всех площадок к справочнику. Подписка площадки идёт отдельной задачей: пересборка её не ждёт
/// (подписка DEX с поиском пулов под темпом узла длится минуты — раньше она держала всю пересборку) и не запускает
/// вторую, пока идёт первая (sub_state.rs). Принимает: ядро, включённые площадки. Ошибок нет — сбой подписки пишется
/// в журнал одной записью на причину и повторяется по следующему толчку пересборки (rebuild.rs).
pub async fn sync(core: &Core, enabled: &[(VenueId, VenueKind)]) {
    let all = rd(&core.inner.kinds).clone();
    for (venue, kind) in all {
        let want = if enabled.iter().any(|(v, _)| v == &venue) {
            lk(&core.inner.subs).hold(&venue, false);
            wanted(core, &venue, kind)
        } else {
            // Решение 196: выключенная площадка с идущими сделками держит поток до их закрытия (feed_hold.rs).
            let (want, held) = super::feed_hold::want_disabled(core, &venue);
            super::feed_hold::mark(core, &venue, held).await;
            want
        };
        if !lk(&core.inner.subs).begin(&venue, &want) {
            continue;
        }
        tokio::spawn(subscribe_one(core.clone(), venue, want));
    }
}

/// Подписка одной площадки и её итог: журнал (успех — всегда, сбой — на новую причину), после успеха — толчок
/// пересборки (список мог измениться, пока шла подписка; тот же список — ничего не делается). Ошибок нет.
async fn subscribe_one(core: Core, venue: VenueId, want: Vec<Asset>) {
    let deps = &core.inner.deps;
    let res = match deps.gw.get(&venue).await {
        Ok(v) => {
            v.subscribe_market(want.clone(), core.inner.market_tx.clone())
                .await
        }
        Err(e) => Err(e),
    };
    let ok = res.is_ok();
    let note = lk(&core.inner.subs).done(&venue, want, res.map_err(|e| e.message));
    let (level, msg) = match note {
        Note::Subscribed(n) => (
            Level::Info,
            format!("Подписка на рыночные данные: {n} монет"),
        ),
        Note::Failed(why) => (
            Level::Warn,
            format!(
                "Подписка не удалась: {why} — повтор при следующей пересборке или изменении настроек"
            ),
        ),
        Note::Same => (Level::Info, String::new()),
    };
    if !msg.is_empty() {
        deps.log(level, "core.subscribe", Some(&venue.0), msg, None)
            .await;
    }
    if ok {
        core.request_rebuild();
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
fn intermediate_assets(core: &Core, venue: Option<&VenueId>) -> Vec<Asset> {
    let s = &core.inner.deps.settings;
    let nets = rd(&core.inner.networks);
    // Решение 211: бирже — тикер промежуточной монеты в ЕЁ справочнике сетей (не знает адреса — не подписывается);
    // DEX — тикер любой биржи (у DEX тикер только показной).
    let name = |a| match venue {
        Some(v) => nets.get(v).and_then(|l| {
            l.iter()
                .find(|i| i.asset.bsc_contract == Some(a))
                .map(|i| i.asset.ticker.clone())
        }),
        None => super::intermediate::ticker_of(&nets, a),
    };
    let mut out = vec![];
    for (v, _) in rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
    {
        if let super::intermediate::Intermediate::On(list) = super::intermediate::read(s, v) {
            for a in list {
                if let Some(t) = name(a) {
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
