//! Файл: venues/bsc_dex/quote/classic_direct.rs
//! Что делает: классическая ПРЯМАЯ котировка (dex.swap_mode = direct, решения 29, 52) с разведением «исполнимо / только
//! сигнал» (решение 121, часть B): котировка-на-сделку берёт только пулы с путём исполнения (family.tradable, не форк —
//! форк исполняется только графом бандлом, ревью #86). Неисполнимые пулы (THENA Fusion, Solidly, Slipstream, DODO,
//! KyberSwap Elastic — tradable = нет) не выигрывают котировку как Direct: раньше такой пул мог стать маршрутом
//! сделки и упасть на plan::build (нет Universal Router) — после покупки на бирже это алерт продажи.
//! Сигнал их по-прежнему видит, как граф (решение 84, вариант Б): если лучшая котировка по ВСЕМ не-форк пулам идёт через
//! неисполнимый пул и лучше исполнимой — её маршрут ложится в signal_route (executable = нет, причина) для карточки,
//! а цена и сделка — по исполнимой. Исполнимого пула нет совсем — NoRoute с причиной «только сигнал».
//! Кто вызывает: quote/mod.rs (Quoting::quote, режим direct), quote/smart_quote.rs (прямая сделка вне умного роутера).
//! От чего зависит: quote/direct_route.rs, quote/pick.rs, quote/via.rs, quote/steps.rs, pools/registry.rs, pools/mids.rs.
//! Настройки: своих нет (через вызываемые — dex.*, решение 73).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, DexRoute, Quote};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::steps::Names;
use super::via::Via;
use super::{direct_route, pick};

/// Пул идёт в котировку-на-сделку прямого пути: есть путь исполнения и не форк. Ошибок нет.
pub fn tradable(e: &Entry) -> bool {
    e.family.tradable && !e.family.fork
}

/// Прямая котировка с разведением «исполнимо / только сигнал». Принимает: расчёт путей, тикеры, монету, количество,
/// пулы монеты с USDT, промежуточные с decimals, фильтр вызывающего (умный режим — только пулы вне умного роутера;
/// прямой — все), цену газа. Возвращает: котировку по исполнимым пулам (signal_route —
/// лучший неисполнимый маршрут, если он лучше). Ошибки: как direct_route::quote; NoRoute «только сигнал».
#[allow(clippy::too_many_arguments)]
pub async fn quote(
    v: &Via<'_>,
    names: &Names<'_>,
    asset: &Asset,
    qty: Decimal,
    entries: Vec<Entry>,
    mids: &[(Mid, u8)],
    base: &(dyn Fn(&Entry) -> bool + Sync),
    gas: U256,
) -> GatewayResult<Quote> {
    let q = v.q;
    let ctx = &q.ctx;
    // Сигналу виден любой не-форк пул фильтра (форк ведёт граф); сделке — только с путём исполнения.
    let visible = |e: &Entry| base(e) && !e.family.fork;
    let exec_keep = |e: &Entry| base(e) && tradable(e);
    // Есть ли у монеты неисполнимые не-форк пулы (с USDT или с промежуточной) — только тогда нужен расчёт сигнала.
    let signal_only = |e: &Entry| visible(e) && !tradable(e);
    let has_signal_only = entries.iter().any(signal_only)
        || mids
            .iter()
            .any(|(m, _)| q.reg.by_pair(v.coin, m.address).iter().any(signal_only));
    let exec =
        direct_route::quote(v, names, asset, qty, entries.clone(), mids, &exec_keep, gas).await;
    if !has_signal_only {
        return exec;
    }
    let sig = direct_route::quote(v, names, asset, qty, entries, mids, &visible, gas).await;
    match (exec, sig) {
        (Ok(mut t), Ok(s)) => {
            let best = pick::pick(q, t.clone(), s.clone(), v.side, v.usdt_dec).await;
            if best.route != t.route
                && let Some(r) = s.route
            {
                t.signal_route = Some(signal(q, r));
            }
            Ok(t)
        }
        (Err(e), Ok(s)) if e.code == ErrorCode::NoRoute => {
            let note = s.route.map(|r| families(q, &r)).unwrap_or_default();
            Err(ctx.err(
                ErrorCode::NoRoute,
                format!(
                    "{}: исполнимого пула нет — лучший пул без пути исполнения ({note}): только сигнал, авто-сделкой \
                     не исполняется; исполнимые: {}",
                    asset.ticker, e.message
                ),
            ))
        }
        (exec, _) => exec,
    }
}

/// Маршрут-сигнал: неисполним как есть, с причиной (видна в карточке сигнала, решение 74). Ошибок нет.
fn signal(q: &super::Quoting, mut r: DexRoute) -> DexRoute {
    r.exec_note = Some(format!(
        "маршрут через пул без пути исполнения ({}) — это сигнал, авто-сделкой не исполняется",
        families(q, &r)
    ));
    r.executable = false;
    r
}

/// Виды пулов маршрута без пути исполнения (id DEX через запятую). Ошибок нет.
fn families(q: &super::Quoting, r: &DexRoute) -> String {
    let mut ids: Vec<String> = Vec::new();
    for h in r.legs.iter().flat_map(|l| &l.hops) {
        let bad = q.reg.get(&h.pool).is_none_or(|e| !tradable(&e));
        if bad && !ids.contains(&h.family) {
            ids.push(h.family.clone());
        }
    }
    ids.join(", ")
}
