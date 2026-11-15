//! Файл: venues/bsc_dex/quote/graph_seq.rs
//! Что делает: доли графового маршрута с ОБЩИМИ пулами — на последовательных состояниях пулов в порядке исполнения
//! (решение 223; живой случай FLUX 08.11: четыре доли через один пул V2 считались каждая на исходном состоянии пула,
//! как независимая ликвидность — сумма выходов завышена, исполнитель провёл доли подряд через тот же пул → minOut).
//! Модель исполнения (docs/notes/graph-shared-pools-2026-11-08.md): исполнитель ArbExecutor проводит доли по порядку
//! маршрута, доля за долей, каждая — своим обменом в каждом своём пуле (exec/executor/call.rs); бандл — тоже по порядку
//! внутри роутер-группы, а доли с общим пулом всегда в одной группе (пул задаёт роутер) — порядок между ними тот же.
//! Поэтому доля k считается на состоянии общего пула ПОСЛЕ долей 1..k−1 (quote/pool_after.rs), её выход — нашей
//! математикой (quote/route_calc.rs, без изменений). Состояние пула после доли — по цепочке её обменов точным входом от
//! входа доли (как исполнит сеть: вход шага — сумма доли или весь баланс выхода прошлого шага после крючка); у продажи эта
//! цепочка совпадает с route_calc до единицы, у покупки (route_calc считает «с конца») — то, что сеть исполнит на вход доли.
//! Доля без общих пулов не меняется и никого не меняет — считается и остаётся как раньше (побайтно).
//! Общий пул вида без расчёта состояния после обмена (pool_after::appliable = нет) — маршрут не считается (NoRoute), сплит
//! такую вторую долю не открывает (quote/graph_split.rs).
//! Кто вызывает: quote/graph_split.rs (сплит и итог), quote/verify_decide.rs (свежие доли и минимумы),
//! exec/dry_paths.rs и exec/bundle_trade.rs (живые доли перед сделкой — resequence).
//! От чего зависит: quote/route_calc.rs (Leg, eval, keep), quote/pool_after.rs, quote/pool_quote.rs, quote/pool_tax.rs
//! (coin_keep), quote/path.rs, num.rs.
//! Настройки: нет своих; через вызываемые — dex.v2_fee_pct [вид пула V2].

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;

use crate::core::types::{PoolRef, PoolState, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::path::Path;
use super::route_calc::{self, Leg, keep};
use super::{pool_after, pool_quote, pool_tax};

/// Пулы, через которые идут две доли и больше. Принимает: ноги долей. Ошибок нет.
pub fn shared(shares: &[&[Leg]]) -> HashSet<PoolRef> {
    let mut seen: HashSet<&PoolRef> = HashSet::new();
    let mut out = HashSet::new();
    for legs in shares {
        // Пул внутри одной доли не повторяется (путь без повторов токенов), но берём множество — на всякий случай.
        let mine: HashSet<&PoolRef> = legs.iter().map(|l| &l.entry.pool.reference).collect();
        for r in mine {
            if !seen.insert(r) {
                out.insert(r.clone());
            }
        }
    }
    out
}

/// Общий пул, для вида которого состояние после обмена не считается (pool_after::appliable). Возвращает: текст причины
/// или None. Ошибок нет.
pub fn unsupported(shares: &[&[Leg]], common: &HashSet<PoolRef>) -> Option<String> {
    shares.iter().flat_map(|l| l.iter()).find_map(|l| {
        (common.contains(&l.entry.pool.reference) && !pool_after::appliable(&l.entry)).then(|| {
            format!(
                "через пул {} {:?} идут две доли и больше, а состояние этого вида пула после обмена не считается — \
                 доли общего пула не считаются как независимые (решение 223)",
                l.entry.family.title, l.entry.pool.kind
            )
        })
    })
}

/// Доли по порядку исполнения на последовательных состояниях общих пулов. Принимает: окружение, доли (ноги, количество
/// монет), сторону, налоги монеты из проверки (запасные, pool_tax::coin_keep). Возвращает: пути долей в том же порядке;
/// доля без общих пулов — ровно как route_calc::eval на исходном состоянии. Ошибки: NoRoute (общий пул вида без расчёта
/// состояния, доля не исполнима на сдвинутом пуле), как у route_calc::eval.
pub fn eval(
    ctx: &Ctx,
    shares: &[(&[Leg], Decimal)],
    side: Side,
    t: &TokenTaxes,
) -> GatewayResult<Vec<Path>> {
    let legs: Vec<&[Leg]> = shares.iter().map(|(l, _)| *l).collect();
    let common = shared(&legs);
    if let Some(w) = unsupported(&legs, &common) {
        return Err(ctx.err(ErrorCode::NoRoute, w));
    }
    let mut after: HashMap<PoolRef, PoolState> = HashMap::new();
    let mut out = Vec::with_capacity(shares.len());
    for (legs, qty) in shares {
        if !legs
            .iter()
            .any(|l| common.contains(&l.entry.pool.reference))
        {
            let tax_keep = pool_tax::coin_keep(legs, side, t);
            out.push(route_calc::eval(ctx, legs, side, *qty, tax_keep)?);
            continue;
        }
        let cur = on_state(legs, &after);
        let tax_keep = pool_tax::coin_keep(&cur, side, t);
        let p = route_calc::eval(ctx, &cur, side, *qty, tax_keep)?;
        advance(ctx, &cur, &p, &common, &mut after)?;
        out.push(p);
    }
    Ok(out)
}

/// Пересчёт уже посчитанных долей (живые пути перед сделкой): доли с общими пулами — заново по порядку на
/// последовательных состояниях (eval), остальные — как есть. Без общих пулов — ничего не меняет. Принимает: окружение,
/// сторону, налоги, доли (путь, количество монет). Ошибки: как eval.
pub fn resequence(
    ctx: &Ctx,
    side: Side,
    t: &TokenTaxes,
    shares: &mut [(Path, Decimal)],
) -> GatewayResult<()> {
    let legs: Vec<Vec<Leg>> = shares.iter().map(|(p, _)| legs_of(p)).collect();
    let refs: Vec<&[Leg]> = legs.iter().map(Vec::as_slice).collect();
    let common = shared(&refs);
    if common.is_empty() {
        return Ok(());
    }
    // Только доли с общими пулами, в исходном порядке: прочие с ними не пересекаются и остаются как посчитаны (их расчёт
    // мог идти не через route_calc — exec/live.rs, один пул).
    let idx: Vec<usize> = (0..refs.len())
        .filter(|&i| {
            refs[i]
                .iter()
                .any(|l| common.contains(&l.entry.pool.reference))
        })
        .collect();
    let input: Vec<(&[Leg], Decimal)> = idx.iter().map(|&i| (refs[i], shares[i].1)).collect();
    let fresh = eval(ctx, &input, side, t)?;
    for (i, p) in idx.into_iter().zip(fresh) {
        shares[i].0 = p;
    }
    Ok(())
}

/// Ноги пути (пул, допуск, токены и decimals каждого хопа) — для пересчёта. Ошибок нет.
pub fn legs_of(p: &Path) -> Vec<Leg> {
    p.hops
        .iter()
        .map(|h| Leg {
            entry: h.entry.clone(),
            usable: h.usable.clone(),
            token_in: h.token_in,
            token_out: h.token_out,
            dec_in: h.dec_in,
            dec_out: h.dec_out,
        })
        .collect()
}

/// Ноги доли на текущих состояниях общих пулов (после прошлых долей). Ошибок нет.
fn on_state(legs: &[Leg], after: &HashMap<PoolRef, PoolState>) -> Vec<Leg> {
    legs.iter()
        .map(|l| {
            let mut l = l.clone();
            if let Some(s) = after.get(&l.entry.pool.reference) {
                l.entry.pool.state = s.clone();
            }
            l
        })
        .collect()
}

/// Провести долю по её пулам, как исполнит сеть: точный вход от входа доли (первый хоп пути), выход хопа после крючка —
/// вход следующего; у общих пулов записать состояние после обмена. Ошибки: NoRoute (обмен не исполним целиком,
/// вид пула), BadResponse (суммы), NotConfigured (комиссия V2).
fn advance(
    ctx: &Ctx,
    legs: &[Leg],
    p: &Path,
    common: &HashSet<PoolRef>,
    after: &mut HashMap<PoolRef, PoolState>,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма доли не переводится в единицы токена",
        )
    };
    let mut amount = p
        .hops
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "доля без пулов"))?
        .swapped
        .amount_in;
    for (i, leg) in legs.iter().enumerate() {
        let r = &leg.entry.pool.reference;
        let s = if common.contains(r) {
            let (s, st) = pool_after::apply(ctx, &leg.entry, leg.token_in, amount)?;
            after.insert(r.clone(), st);
            s
        } else {
            pool_quote::swap(ctx, &leg.entry, leg.token_in, true, amount)?
        };
        if !s.complete {
            return Err(ctx.err(ErrorCode::NoRoute, "обмен доли не исполним целиком"));
        }
        if i + 1 < legs.len() {
            let got = num::to_decimal(s.amount_out, leg.dec_out).ok_or_else(bad)?
                * keep(leg.usable.hook_pct);
            amount = num::to_units(got, leg.dec_out).ok_or_else(bad)?;
        }
    }
    Ok(())
}
