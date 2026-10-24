//! Файл: venues/bsc_dex/quote/smart_ways.rs
//! Что делает: кандидаты деления умного маршрута (решения 38, 52): пулы PancakeSwap с USDT (монета с налогом — только V2)
//! и по каждой промежуточной монете одна лучшая пара пулов PancakeSwap; расчёт кандидата на количество.
//! Кто вызывает: quote/smart.rs.
//! От чего зависит: quote/direct.rs, quote/via.rs, quote/via_pools.rs, quote/select.rs, quote/path.rs, pools/families.rs.
//! Настройки: через вызываемые — см. их шапки.

use rust_decimal::Decimal;

use crate::core::types::{PoolKind, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::families::PANCAKESWAP;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::direct::{self, Candidate};
use super::path::Path;
use super::select::{self, Usable};
use super::via::{Ok2, Via};

/// Кандидат деления: один пул с USDT или пара пулов через промежуточную монету.
pub enum Way {
    One(Box<(Entry, Usable)>),
    Two(Box<(Mid, u8, Ok2, Ok2)>),
}

/// Кандидат и сколько монет ему уже отдано.
pub struct Slot {
    pub way: Way,
    pub qty: Decimal,
    pub got: Option<Path>,
}

/// Пулы PancakeSwap с USDT, пригодные для умного маршрута. Возвращает: кандидаты и причины отказа по остальным.
pub fn pools(
    q: &Quoting,
    entries: &[Entry],
    side: Side,
    taxed: bool,
    usdt_dec: u8,
) -> (Vec<Way>, Vec<String>) {
    let mut ok = Vec::new();
    let mut why = Vec::new();
    for e in entries.iter().filter(|e| e.family.dex == PANCAKESWAP) {
        if taxed && e.pool.kind != PoolKind::V2 {
            why.push(format!(
                "{}: у монеты налог — в умном маршруте только V2",
                e.family.title
            ));
            continue;
        }
        match select::usable(q, e, side, usdt_dec) {
            Ok(u) => ok.push(Way::One(Box::new((e.clone(), u)))),
            Err(w) => why.push(format!("{}: {w}", e.family.title)),
        }
    }
    (ok, why)
}

/// Пары через промежуточные монеты: по каждой — лучшая пара пулов PancakeSwap на весь объём. Причины — в why.
pub fn pairs(v: &Via<'_>, mids: &[(Mid, u8)], qty: Decimal, why: &mut Vec<String>) -> Vec<Way> {
    let pcs = |e: &Entry| e.family.dex == PANCAKESWAP;
    let mut out = Vec::new();
    for (m, dec) in mids {
        let (p1, p2) = match v.pools(m, &pcs) {
            Ok(x) => x,
            Err(w) => {
                why.push(w);
                continue;
            }
        };
        let mut best: Option<(Path, Ok2, Ok2)> = None;
        for a in &p1 {
            for b in &p2 {
                if let Ok(p) = v.calc(m, *dec, a, b, qty)
                    && best
                        .as_ref()
                        .is_none_or(|x| super::via_pools::better(&p, &x.0, v.side))
                {
                    best = Some((p, a.clone(), b.clone()));
                }
            }
        }
        match best {
            Some((_, a, b)) => out.push(Way::Two(Box::new((m.clone(), *dec, a, b)))),
            None => why.push(format!("через {}: пара пулов не исполняет объём", m.ticker)),
        }
    }
    out
}

/// Расчёт кандидата на количество. Ошибки: как у direct::calc / via::calc.
pub fn calc(v: &Via<'_>, w: &Way, qty: Decimal) -> GatewayResult<Path> {
    match w {
        Way::One(x) => {
            let (e, u) = (&x.0, &x.1);
            let (swapped, coins, usdt) = direct::calc(
                &v.q.ctx,
                e,
                u,
                v.side,
                qty,
                v.taxes,
                (v.coin_dec, v.usdt_dec),
            )?;
            if !swapped.complete {
                return Err(v
                    .q
                    .ctx
                    .err(ErrorCode::NoRoute, "пул не исполняет шаг целиком"));
            }
            let c = Candidate {
                entry: e.clone(),
                usable: u.clone(),
                swapped,
                coins,
                usdt,
            };
            Path::one(&v.q.ctx, c, v.side, (v.coin_dec, v.usdt_dec))
        }
        Way::Two(x) => v.calc(&x.0, x.1, &x.2, &x.3, qty),
    }
}
