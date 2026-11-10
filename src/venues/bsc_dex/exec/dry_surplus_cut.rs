//! Файл: venues/bsc_dex/exec/dry_surplus_cut.rs
//! Что делает: поправка выхода симуляции на излишек пары, созданный пополнением (решение 252, дополнение «dry_fund»,
//! Тигран 10.11: вариант «б»; замер — exec/dry_surplus.rs). Для каждой доли с приростом излишка у пары V2 первого
//! шага: вход пары в симуляции I (рост резерва токена за обмен), выход o1 (падение резерва встречного) — сверка
//! формулой пары на резервах той же симуляции (не сошлось — отказ, поправка не была бы точной); выход без прироста
//! o1' = getAmountOut(I − прирост). Доля из одного шага — уменьшение выхода = o1 − o1'. Дальше по доле — хвост
//! (шаги 2…N, пулы без пополняемого токена — пополнение их не трогает) квотером каждого шага на базовом блоке
//! симуляции (eth_simulateV1, тот же Base; виртуальный квотер — подменой кода, решение 186) для o1 и o1' — уменьшение =
//! хвост(o1) − хвост(o1') (exec/dry_surplus_tail.rs). Итог: выход симуляции − сумма уменьшений; Sim.surplus — для строки журнала (line).
//! Пул Solidly / PMM первого шага с приростом баланса — отказ (формулы поправки нет). Прироста нет (обычная монета) —
//! ничего не меняется.
//! Кто вызывает: exec/dry_sim.rs (apply); exec/{presell, dry_swap, cycle_dry}.rs (line).
//! От чего зависит: exec/{dry_surplus, dry_surplus_tail, dry_build, dry_sim}.rs, check/probe_calls.rs,
//! quote/pool_quote.rs, math/v2.rs, num.rs.
//! Настройки: через вызываемые — dex.v2_fee_pct [DEX], dex.router / dex.quoter [DEX], dex.quoter_virtual [DEX].

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef};

use super::super::check::probe_calls as pc;
use super::super::check::simulate::CallResult;
use super::super::ctx::Ctx;
use super::super::math::v2;
use super::super::num;
use super::super::quote::pool_quote;
use super::dry_build::{Ready, entries};
use super::dry_sim::Sim;
use super::dry_surplus::{Watch, first_hop, grown, reserves};
use super::dry_surplus_tail::tail;

/// Излишек от пополнения в минимальных единицах: прирост излишка пар (токен входа), сумма пополнения, на сколько
/// уменьшен выход (токен выхода).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surplus {
    pub coins: U256,
    pub funded: U256,
    pub cut: U256,
}

/// Поправка выхода. Принимает: окружение, сделку, пулы замера, итоги симуляции, сумму пополнения, итог (sim.out уже
/// прочитан). Меняет: sim.out (минус уменьшение), sim.surplus; отказ — sim.reason. Ошибок нет (сбой — причина).
pub async fn apply(
    ctx: &Ctx,
    r: &Ready,
    ws: &[Watch],
    res: &[CallResult],
    funded: U256,
    sim: &mut Sim,
) {
    match total(ctx, r, ws, res).await {
        Ok((coins, _)) if coins.is_zero() => {}
        Ok((coins, cut)) => {
            sim.out = sim.out.map(|o| o.saturating_sub(cut));
            sim.surplus = Some(Surplus { coins, funded, cut });
        }
        Err(why) => sim.reason = Some(format!("излишек пары от пополнения: {why}")),
    }
}

/// Сумма прироста и уменьшения выхода по всем пулам замера. Err — текст причины.
async fn total(
    ctx: &Ctx,
    r: &Ready,
    ws: &[Watch],
    res: &[CallResult],
) -> Result<(U256, U256), String> {
    let (mut coins, mut cut) = (U256::ZERO, U256::ZERO);
    let at = |i: Option<usize>| i.and_then(|k| res.get(k));
    for w in ws {
        let bal = |s: usize| at(w.bal[s]).and_then(pc::u256);
        let (Some(b0), Some(b1)) = (bal(0), bal(1)) else {
            return Err(format!("баланс пула {:#x} не прочитан", w.pool));
        };
        if w.kind != PoolKind::V2 {
            let d = grown((b0, U256::ZERO), (b1, U256::ZERO));
            if !d.is_zero() {
                return Err(format!(
                    "баланс пула {:#x} (вид {:?}) вырос на {d} ед. от пополнения — вход этого вида = баланс − резерв, \
                     формулы поправки для него нет; продажа симуляцией не подтверждена",
                    w.pool, w.kind
                ));
            }
            continue;
        }
        let rs = |s: usize| at(w.res[s]).and_then(|c| reserves(c, w.zero_in));
        let (Some(r0), Some(r1), Some(r2)) = (rs(0), rs(1), rs(2)) else {
            return Err(format!("резервы пары {:#x} не прочитаны", w.pool));
        };
        let d = grown((b0, r0.0), (b1, r1.0));
        if d.is_zero() {
            continue;
        }
        let same = PoolRef::Contract(w.pool);
        if entries(&r.legs)
            .iter()
            .filter(|e| e.pool.reference == same)
            .count()
            != 1
        {
            return Err(format!(
                "пара {:#x} встречается в маршруте не один раз — вход каждой доли не отделить",
                w.pool
            ));
        }
        let steps = &r.legs[w.leg];
        let fee = pool_quote::v2_fee_pct(ctx, &steps[0].entry)
            .ok()
            .and_then(v2::fee_ratio)
            .ok_or_else(|| "комиссия пары не задана (dex.v2_fee_pct)".to_string())?;
        let (Some(input), Some(seen)) = (r2.0.checked_sub(r1.0), r1.1.checked_sub(r2.1)) else {
            return Err(format!(
                "резервы пары {:#x} после обмена не по направлению продажи",
                w.pool
            ));
        };
        let (o1, o1c) = first_hop(input, d, r1, seen, fee)?;
        let less = if steps.len() == 1 {
            o1 - o1c
        } else {
            let [a, b] = tail(ctx, &steps[1..], r.at, [o1, o1c])
                .await
                .map_err(|e| format!("хвост доли {} квотером: {}", w.leg + 1, e.message))?;
            a.saturating_sub(b)
        };
        coins += d;
        cut += less;
    }
    Ok((coins, cut))
}

/// Строка журнала: «излишек пары от пополнения: N монет (X % перевода пополнения), выход уменьшен на Y <выход>».
/// Принимает: замер, decimals токена входа и выхода, тикер выхода. Ошибок нет (не считается в число — сырые единицы).
pub fn line(s: &Surplus, dec_in: u8, dec_out: u8, out_name: &str) -> String {
    let coins = num::to_decimal(s.coins, dec_in);
    let pct = num::to_decimal(s.coins, 0)
        .zip(num::to_decimal(s.funded, 0))
        .filter(|(_, f)| !f.is_zero())
        .map(|(c, f)| {
            (c / f * Decimal::ONE_HUNDRED)
                .round_dp(4)
                .normalize()
                .to_string()
        })
        .unwrap_or_else(|| "—".into());
    let cut = num::to_decimal(s.cut, dec_out);
    format!(
        "излишек пары от пополнения: {} монет ({pct} % перевода пополнения), выход уменьшен на {} {out_name}",
        coins.map_or(s.coins.to_string(), |c| c.normalize().to_string()),
        cut.map_or(s.cut.to_string(), |c| c.round_dp(6).normalize().to_string()),
    )
}
