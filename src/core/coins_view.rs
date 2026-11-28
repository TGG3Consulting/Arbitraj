//! Файл: core/coins_view.rs
//! Что делает: данные экрана «Монеты и направления» в формате GET /api/coins (src/api/README.md):
//! монета, площадки, налоги и их проверка, чёрный список, пулы с крючками и проверками; плюс
//! причины отсева по площадкам (presence), проблема оценки (problem) и рабочие направления;
//! пулы с USDT и с промежуточными монетами (pair), через какую промежуточную монету допущена (via, решение 52).
//! Строка одной монеты (t84) — та же функция `row`, что у полного списка: строка ровно как в полном списке.
//! Решение 413: deposit_holds — замеры удержания бирж при зачислении монеты (deposit_hold.rs).
//! Кто вызывает: api/coins.rs — GET /api/coins (Core::coins_json) и GET /api/coins/{address} (Core::coin_json).
//! От чего зависит: engine.rs, registry.rs, market_state.rs, blacklist.rs, intermediate.rs, deposit_hold.rs.
//! Решение 432: пулы монеты с USDT — USDT её сети. К1 (арбитр 28.11.2026): с расчётной монетой площадки DEX
//! (settlement_pool::current — на Base USDC, как mid_wake.rs), подпись пары — venue.quote_asset площадки.
//! Настройки: venue.quote_asset [DEX] (читается — подпись пары).

use alloy_primitives::Address;
use serde_json::{Value, json};

use std::collections::HashMap;

use crate::core::types::{ChainId, CheckState, VenueId};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::blacklist;
use super::engine::{Core, rd};
use super::intermediate::{self, Intermediate};
use super::market_state::MarketState;
use super::registry::CoinEntry;
use super::registry_build::Networks;

/// Снимки ядра для строк монет — берутся один раз на весь список (или на одну строку).
struct Ctx<'a> {
    market: &'a MarketState,
    /// К1 (арбитр 28.11.2026): расчётная монета каждой площадки DEX (settlement_pool::current — на Base USDC, на BSC
    /// USDT её сети, как в mid_wake.rs) и подпись пары из venue.quote_asset площадки.
    settle: HashMap<VenueId, (Address, String)>,
    s: &'a Store,
    nets: &'a Networks,
}

/// К1: расчётная монета и подпись пары каждой площадки DEX — до блокировок строк (settlement_pool::current читает свои
/// снимки ядра). Подпись — venue.quote_asset площадки; не задана — адрес монеты. Площадка без расчётной монеты (биржи
/// не подтвердили адрес) — без пулов с ней на экране. Ошибок нет.
fn settle_map(core: &Core) -> HashMap<VenueId, (Address, String)> {
    let s = &core.inner.deps.settings;
    super::chain::dexes(core)
        .into_iter()
        .filter_map(|v| {
            let addr = super::settlement_pool::current(core, &v)?;
            let at = At {
                venue: Some(&v.0),
                coin: None,
            };
            let label = s
                .text("venue.quote_asset", at)
                .unwrap_or_else(|_| format!("{addr:#x}"));
            Some((v, (addr, label)))
        })
        .collect()
}

impl Core {
    /// Список монет для экрана. Возвращает: JSON-массив. Ошибок нет.
    /// Налоги — с первой DEX, где у монеты есть проверка (все проверки — в поле checks).
    pub fn coins_json(&self) -> Value {
        let settle = settle_map(self);
        let coins = rd(&self.inner.coins);
        let market = rd(&self.inner.market);
        let nets = rd(&self.inner.networks);
        let x = Ctx {
            market: &market,
            settle,
            s: &self.inner.deps.settings,
            nets: &nets,
        };
        // Экран — все монеты: рабочие и неподвижные (решение 131, coin_book.rs).
        Value::Array(coins.all().map(|c| row(c, &x)).collect())
    }

    /// Строка одной монеты по адресу контракта (t84, GET /api/coins/{address}) — та же функция строки, что у
    /// coins_json. Вызов без сети выбирает прежнюю строку сети по умолчанию.
    /// Принимает: адрес. Возвращает: строку или None — монеты с таким адресом в справочнике нет. Ошибок нет.
    pub fn coin_json(&self, address: Address) -> Option<Value> {
        self.coin_json_in(address, None)
    }

    /// Строка по контракту и каноническому тегу сети (None для сети по умолчанию).
    pub fn coin_json_in(&self, address: Address, chain: Option<&ChainId>) -> Option<Value> {
        let settle = settle_map(self);
        let coins = rd(&self.inner.coins);
        let c = coins
            .all()
            .find(|c| c.asset.chain.as_ref() == chain && c.asset.contract == Some(address))?;
        let market = rd(&self.inner.market);
        let nets = rd(&self.inner.networks);
        let x = Ctx {
            market: &market,
            settle,
            s: &self.inner.deps.settings,
            nets: &nets,
        };
        Some(row(c, &x))
    }
}

/// Строка монеты экрана (формат — src/api/README.md, «Экран Монеты»). Принимает: запись справочника, снимки ядра.
/// Возвращает: JSON-объект. Ошибок нет.
fn row(c: &CoinEntry, x: &Ctx) -> Value {
    let (market, s, nets) = (x.market, x.s, x.nets);
    let dex: Vec<_> = c.presence.iter().filter(|p| p.info.is_none()).collect();
    let checks: Vec<_> = match c.asset.contract {
        Some(t) => dex
            .iter()
            .filter_map(|p| market.coin_check(&p.venue, t))
            .collect(),
        None => vec![],
    };
    let first = checks.first();
    // Пулы монеты с USDT и (решение 52) с промежуточными монетами площадки; pair — второй токен.
    let mut pools: Vec<Value> = vec![];
    // К1 (арбитр 28.11.2026): расчётная монета площадки DEX (на Base — USDC), а не USDT сети монеты; подпись —
    // venue.quote_asset площадки.
    if let Some(t) = c.asset.contract {
        for p in &dex {
            let Some((u, label)) = x.settle.get(&p.venue) else {
                continue;
            };
            let mut others = vec![(*u, label.clone())];
            if let Intermediate::On(list) = intermediate::read(s, &p.venue) {
                others.extend(list.iter().map(|m| (*m, intermediate::label(nets, *m))));
            }
            for (other, pair) in others {
                for pl in market.usdt_pools(&p.venue, t, other) {
                    let check = market.pool_check(&pl.venue, &pl.reference);
                    pools.push(json!({
                        "venue": pl.venue,
                        "pair": pair,
                        "family": pl.family,
                        "kind": pl.kind,
                        "pool": pl.reference,
                        "fee_pips": pl.fee_pips,
                        "hooks": pl.hooks,
                        "check": check,
                        "admission": blacklist::pool_admission(pl, check, s).err(),
                        // Ревью 4 лесенки (Н3): сторона «не прошла» — пул работает другой стороной.
                        "side_off": blacklist::pool_sides_off(check),
                        "block": pl.block,
                        "received_at": pl.received_at,
                    }));
                }
            }
        }
    }
    // Через какую промежуточную монету допущена монета на каждой DEX (решение 52).
    let via: Vec<Value> = dex
        .iter()
        .filter_map(|p| {
            p.via
                .map(|m| json!({ "venue": p.venue, "via": intermediate::label(nets, m) }))
        })
        .collect();
    json!({
        "asset": c.asset,
        "venues": c.venues(),
        "taxes": first.map(|k| k.taxes.clone()).unwrap_or(CheckState::NotChecked),
        "tax_source": first.map(|k| k.source.clone()),
        "checked_at": first.and_then(|k| k.checked_at),
        "tax_wait": c.wait,
        // Решение 413: замеры удержания бирж при зачислении (deposit_hold.rs) — экран «Монеты», карточка монеты.
        "deposit_holds": super::deposit_hold::of_coin(&c.asset),
        "checks": checks,
        "block": c.block,
        "problem": c.problem,
        "cex_cex": c.cex_cex,
        "presence": c.presence,
        "directions": c.directions(),
        "pools": pools,
        "via": via,
    })
}
