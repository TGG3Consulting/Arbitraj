//! Файл: venues/bsc_dex/exec/cycle_bundle.rs
//! Что делает: сборка бандла свопов для ПОСЛЕДОВАТЕЛЬНОГО цикла DEX ↔ DEX (решения 85, 86 — вариант 2). Цикл
//! base → t1 → … → base исполняется атомарным бандлом (решение 84): по одному execute Universal Router на участок
//! подряд идущих хопов ОДНОГО роутера. ЭТОТ сборщик — для цикла; НЕ exec/bundle.rs (тот группирует ПАРАЛЛЕЛЬНЫЕ доли
//! ноги BingX ↔ DEX в один выходной токен с единым BALANCE_CHECK). Переиспользует нейтральные части: типы
//! exec/bundle.rs (Bundle/BundleTx), exec/plan.rs (Plan/Order, BALANCE_CHECK), exec/chain.rs (Step), общий расчёт
//! quote/route_calc.rs, числа num.rs.
//!
//! Передача промежуточной монеты (без своего контракта, v1):
//! - ВНУТРИ одного Universal Router (участок из нескольких хопов подряд) — цепочка через CONTRACT_BALANCE в одном
//!   execute (нативный многохоп UR, без пыли): первый хоп участка платит кошелёк (Permit2) фиксированной суммой,
//!   следующие берут весь баланс роутера (from_wallet=false → CONTRACT_BALANCE), последний отдаёт кошельку.
//! - МЕЖРОУТЕРНЫЙ переход (выход участка на роутере A → вход участка на роутере B) — вход следующего участка берётся
//!   по ГАРАНТИРОВАННОМУ МИНИМУМУ предыдущего (Permit2 на фиксированную сумму = minOut последнего хопа предыдущего
//!   участка). Перевод всегда проходит: фактический выход ≥ minOut, иначе предыдущий участок откатился бы и весь бандл
//!   не попал в блок (атомарно, газ не горит — решения 40, 43, 84). Избыток (факт − minOut) остаётся ПЫЛЬЮ в
//!   промежуточной монете в кошельке (учёт — dust_tokens; подметание — отдельный follow-up).
//!
//! Per-hop minOut (решение 40) — на СВОЙ выходной токен каждого хопа: ожидаемый выход × (1 − крючок) ×
//! (1 − exec.dex_slippage_pct). Последний участок возвращает базовую монету, поэтому его BALANCE_CHECK — на базу.
//! Каждый участок считается route_calc по СВОЕМУ фиксированному входу (у участков после первого вход = гарантированный
//! минимум предыдущего, меньше пробы), чтобы minOut хопов были по реальному потоку, а не завышены.
//!
//! Симуляция (eth_simulateV1), подпись и отправка (eth_sendBundle), settle очереди — существующими exec/bundle_sim.rs
//! и exec/bundle_send.rs (не меняются): кумулятивная база BALANCE_CHECK каждого участка читается ПОСЛЕ предыдущих
//! транзакций. Повтор при неудаче НЕ делаем (стартовое правило Тиграна).
//! Гейт исполнения dex_dex.execute по умолчанию ВЫКЛ — exec/cycle_gate.rs (здесь реэкспорт); сам сборщик гейт НЕ
//! проверяет (его зовёт exec/cycle_trade.rs). Шаги участка — exec/cycle_segment.rs; последнему хопу последнего участка
//! — граница ядра floor (решение 124: вход + затраты + порог прибыли).
//! Форк-хоп (classic V2 Router) и не-tradable семейство в цикле v1 не собираются — отказ (следующая фаза).
//! Кто вызывает: exec/{cycle_bundle_trade, cycle_dry}.rs (решение 124); тесты.
//! От чего зависит: exec/{bundle, plan, cycle_segment, cycle_gate}.rs, quote/route_calc.rs, num.rs, ctx.rs.
//! Настройки: dex_dex.execute [bsc_dex] (гейт); exec.dex_slippage_pct [bsc_dex] (per-hop minOut); через plan/ur —
//! dex.universal_router, dex.permit2, dex.universal_router_version [DEX], dex.deadline_s, dex.approve_unlimited,
//! dex.permit2_expiration_s [bsc_dex].

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::quote::route_calc::Leg;
use super::bundle::{Bundle, BundleTx};
use super::cycle_gate::guard_ur;
use super::{cycle_segment, plan};

pub use super::cycle_gate::{execute_enabled, require_execute};

/// Один участок цикла (подряд идущие хопы одного Universal Router) — для учёта и проверки.
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    /// Universal Router участка.
    pub router: Address,
    /// Выходной токен участка (у последнего — базовая монета; у прочих — промежуточная, где оседает пыль).
    pub out_token: Address,
    /// Вход участка (единицы входного токена): у первого — проба в базовой монете, у прочих — гарантированный
    /// минимум предыдущего участка (Permit2 на эту фиксированную сумму — межроутерный переход).
    pub input_units: U256,
    /// Гарантированный минимум выхода участка (minOut последнего хопа) — вход следующего участка.
    pub guaranteed_min: U256,
    /// Число хопов участка (больше одного — цепочка через CONTRACT_BALANCE внутри одного execute).
    pub hops: usize,
}

/// Собранный бандл цикла и учёт участков/пыли.
#[derive(Debug)]
pub struct CycleAssembly {
    /// Готовый бандл — шлётся как есть exec/bundle_send::send (решение 84).
    pub bundle: Bundle,
    /// Участки в порядке исполнения (base → … → base).
    pub segments: Vec<SegmentInfo>,
}

impl CycleAssembly {
    /// Промежуточные монеты межроутерных переходов, где оседает пыль (факт − гарантированный минимум). Пусто — весь
    /// цикл внутри одного Universal Router (один участок, пыли нет). Учёт; подметание — отдельный follow-up. Ошибок нет.
    pub fn dust_tokens(&self) -> Vec<Address> {
        let n = self.segments.len();
        self.segments
            .iter()
            .take(n.saturating_sub(1))
            .map(|s| s.out_token)
            .collect()
    }
}

/// Собирает бандл цикла без границы ядра (минимумы — только по допуску). Принимает и ошибки: как build_floor.
pub async fn build(
    ctx: &Ctx,
    legs: &[Leg],
    input: Decimal,
    wallet: Address,
    first_nonce: u64,
    now: u64,
    all_approvals: bool,
) -> GatewayResult<CycleAssembly> {
    let at = (wallet, first_nonce, now, all_approvals);
    build_floor(ctx, legs, (input, U256::ZERO), at).await
}

/// Собирает бандл цикла. Принимает: ноги цикла в порядке исполнения (base → … → base, из quote/cycle::legs_for_cycle
/// или quote/cycle_legs.rs), (вход в базовой монете, граница выхода последнего хопа в единицах базовой — U256::ZERO без
/// границы), (кошелёк, первый nonce, время блока в секундах, нужны ли все разрешения: true — не ходить на узел, сборка и
/// тесты; false — только недостающие, узел). Возвращает: бандл и учёт пыли.
/// Ошибки: NoRoute (пусто, хоп не исполним бандлом v1), QuoteMismatch (минимум 0), NotConfigured (адреса/срок),
/// BadResponse (числа), Internal (план, форматы), узел (all_approvals=false).
pub async fn build_floor(
    ctx: &Ctx,
    legs: &[Leg],
    (input, floor): (Decimal, U256),
    (wallet, first_nonce, now, all_approvals): (Address, u64, u64, bool),
) -> GatewayResult<CycleAssembly> {
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "пустой цикл — бандл не собирается"));
    }
    for l in legs {
        guard_ur(ctx, l)?;
    }
    // Участки — подряд идущие хопы ОДНОГО Universal Router (строгий порядок цикла сохраняется).
    let mut segments: Vec<(Address, Vec<Leg>)> = Vec::new();
    for l in legs {
        let router = ctx.dex_addr("dex.universal_router", l.entry.family.id)?;
        match segments.last_mut() {
            Some((r, seg)) if *r == router => seg.push(l.clone()),
            _ => segments.push((router, vec![l.clone()])),
        }
    }
    let mut txs: Vec<BundleTx> = Vec::new();
    let mut nonce = first_nonce;
    let mut infos: Vec<SegmentInfo> = Vec::with_capacity(segments.len());
    // Вход первого участка — проба в базовой монете; дальше — гарантированный минимум предыдущего участка.
    let mut input_units = num::to_units(input, legs[0].dec_in).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "вход цикла не переводится в единицы базовой монеты",
        )
    })?;
    let last = segments.len().saturating_sub(1);
    for (i, (router, seg)) in segments.iter().enumerate() {
        let f = if i == last { floor } else { U256::ZERO };
        let (steps, mins) = cycle_segment::steps(ctx, seg, input_units, f)?;
        // План участка: Permit2 на вход участка (фиксированная сумма) + команды + BALANCE_CHECK на выход участка
        // (последний участок — базовая монета). Разрешения вынимаем в отдельные транзакции бандла.
        let mut p = plan::build(ctx, &steps, wallet, now, all_approvals).await?;
        for (to, data) in std::mem::take(&mut p.approvals) {
            txs.push(BundleTx::Approve { to, data, nonce });
            nonce += 1;
        }
        let guaranteed = *mins.last().expect("участок не пуст");
        txs.push(BundleTx::Swap {
            plan: p,
            nonce,
            legs_min: mins,
        });
        nonce += 1;
        infos.push(SegmentInfo {
            router: *router,
            out_token: seg.last().expect("участок не пуст").token_out,
            input_units,
            guaranteed_min: guaranteed,
            hops: seg.len(),
        });
        // Межроутерный переход: вход следующего участка = гарантированный минимум этого.
        input_units = guaranteed;
    }
    Ok(CycleAssembly {
        bundle: Bundle {
            txs,
            first_nonce,
            swaps: segments.len(),
        },
        segments: infos,
    })
}
