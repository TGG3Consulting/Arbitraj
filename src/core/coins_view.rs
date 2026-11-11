//! Файл: core/coins_view.rs
//! Что делает: данные экрана «Монеты и направления» в формате GET /api/coins (src/api/README.md):
//! монета, площадки, налоги и их проверка, чёрный список, пулы с крючками и проверками; плюс
//! причины отсева по площадкам (presence), проблема оценки (problem) и рабочие направления;
//! пулы с USDT и с промежуточными монетами (pair), через какую промежуточную монету допущена (via, решение 52).
//! Строка одной монеты (t84) — та же функция `row`, что у полного списка: строка ровно как в полном списке.
//! Кто вызывает: api/coins.rs — GET /api/coins (Core::coins_json) и GET /api/coins/{address} (Core::coin_json).
//! От чего зависит: engine.rs, registry.rs, market_state.rs, blacklist.rs, intermediate.rs.
//! Настройки: нет.

use alloy_primitives::Address;
use serde_json::{Value, json};

use crate::core::types::CheckState;
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
    usdt: Option<Address>,
    s: &'a Store,
    nets: &'a Networks,
}

impl Core {
    /// Список монет для экрана. Возвращает: JSON-массив. Ошибок нет.
    /// Налоги — с первой DEX, где у монеты есть проверка (все проверки — в поле checks).
    pub fn coins_json(&self) -> Value {
        let coins = rd(&self.inner.coins);
        let market = rd(&self.inner.market);
        let usdt = rd(&self.inner.usdt).clone().ok();
        let nets = rd(&self.inner.networks);
        let x = Ctx {
            market: &market,
            usdt,
            s: &self.inner.deps.settings,
            nets: &nets,
        };
        // Экран — все монеты: рабочие и неподвижные (решение 131, coin_book.rs).
        Value::Array(coins.all().map(|c| row(c, &x)).collect())
    }

    /// Строка одной монеты по адресу контракта (t84, GET /api/coins/{address}) — та же функция строки, что у
    /// coins_json. Один адрес — одна запись справочника (решение 211, core/registry_build.rs: ключ — адрес).
    /// Принимает: адрес. Возвращает: строку или None — монеты с таким адресом в справочнике нет. Ошибок нет.
    pub fn coin_json(&self, address: Address) -> Option<Value> {
        let coins = rd(&self.inner.coins);
        let c = coins
            .all()
            .find(|c| c.asset.bsc_contract == Some(address))?;
        let market = rd(&self.inner.market);
        let usdt = rd(&self.inner.usdt).clone().ok();
        let nets = rd(&self.inner.networks);
        let x = Ctx {
            market: &market,
            usdt,
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
    let checks: Vec<_> = match c.asset.bsc_contract {
        Some(t) => dex
            .iter()
            .filter_map(|p| market.coin_check(&p.venue, t))
            .collect(),
        None => vec![],
    };
    let first = checks.first();
    // Пулы монеты с USDT и (решение 52) с промежуточными монетами площадки; pair — второй токен.
    let mut pools: Vec<Value> = vec![];
    if let (Some(t), Some(u)) = (c.asset.bsc_contract, x.usdt) {
        for p in &dex {
            let mut others = vec![(u, "USDT".to_string())];
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
