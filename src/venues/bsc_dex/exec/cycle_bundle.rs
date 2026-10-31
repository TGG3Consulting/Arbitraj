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
//! Гейт исполнения dex_dex.execute по умолчанию ВЫКЛ (ценность «всё под контролем человека»): выключено — цикл только
//! сигнал (require_execute). Сам сборщик гейт НЕ проверяет (его зовёт слой исполнения); сборка чистая и тестируется.
//! Форк-хоп (classic V2 Router) и не-tradable семейство в цикле v1 не собираются — отказ (следующая фаза).
//! Кто вызывает: слой исполнения DEX ↔ DEX (следующий стык); тесты.
//! От чего зависит: exec/bundle.rs, exec/plan.rs, exec/chain.rs, exec/ur.rs, quote/route_calc.rs, num.rs,
//! pools/families.rs (Family.tradable/fork), ctx.rs.
//! Настройки: dex_dex.execute [bsc_dex] (гейт); exec.dex_slippage_pct [bsc_dex] (per-hop minOut); через plan/ur —
//! dex.universal_router, dex.permit2, dex.universal_router_version [DEX], dex.deadline_s, dex.approve_unlimited,
//! dex.permit2_expiration_s [bsc_dex].

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::quote::route_calc::{self, Leg};
use super::bundle::{Bundle, BundleTx};
use super::chain::Step;
use super::plan::{self, Order};
use super::ur::{self, CONTRACT_BALANCE};

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

/// Включено ли АВТО-ИСПОЛНЕНИЕ циклов DEX ↔ DEX (решение 86). Не задано — выключено (как dex_dex.enabled): цикл
/// только сигнал, «всё под контролем человека». Ошибок нет.
pub fn execute_enabled(ctx: &Ctx) -> bool {
    ctx.flag("dex_dex.execute").unwrap_or(false)
}

/// Гейт исполнения цикла: выключено (по умолчанию) — сделки нет (сигнал). Ошибки: NoRoute — гейт закрыт.
pub fn require_execute(ctx: &Ctx) -> GatewayResult<()> {
    if execute_enabled(ctx) {
        Ok(())
    } else {
        Err(ctx.err(
            ErrorCode::NoRoute,
            "авто-исполнение циклов DEX ↔ DEX выключено (dex_dex.execute) — цикл только сигнал",
        ))
    }
}

/// Хоп цикла исполним бандлом v1 (через Universal Router). Ошибки: NoRoute — не-tradable семейство или форк (classic
/// V2 Router — следующая фаза); NotConfigured — у DEX не задан dex.universal_router.
fn guard_ur(ctx: &Ctx, leg: &Leg) -> GatewayResult<()> {
    let f = leg.entry.family;
    if !f.tradable {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "цикл проходит через {} без своего пути исполнения — это сигнал, бандлом не исполняется",
                f.id
            ),
        ));
    }
    if f.fork {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "хоп цикла через форк {} (свой classic V2 Router) в бандле цикла v1 не собирается — следующая фаза",
                f.id
            ),
        ));
    }
    ctx.dex_addr("dex.universal_router", f.id)?;
    Ok(())
}

/// Шаги одного участка (подряд идущие хопы одного Universal Router) и их minOut. Участок считается route_calc по
/// своему фиксированному входу `input_units` (та же математика, что у продажи цикла — каждый пул принимает точный
/// вход). Первый хоп платит кошелёк суммой input_units, следующие — весь баланс роутера (CONTRACT_BALANCE), последний
/// отдаёт кошельку. Возвращает: шаги, minOut по хопам. Ошибки: NoRoute (пусто/шаг не исполним целиком), BadResponse
/// (числа), QuoteMismatch (минимум хопа 0), NotConfigured (exec.dex_slippage_pct), Internal (разные форматы UR).
fn segment_steps(
    ctx: &Ctx,
    legs: &[Leg],
    input_units: U256,
) -> GatewayResult<(Vec<Step>, Vec<U256>)> {
    let input = num::to_decimal(input_units, legs[0].dec_in).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "вход участка цикла не переводится в число",
        )
    })?;
    // Один формат входов UR на участок (как L3 exec/bundle.rs): plan::build кодирует все хопы форматом первого.
    let fmt = ur::format(ctx, &legs[0].entry)?;
    for l in &legs[1..] {
        if ur::format(ctx, &l.entry)? != fmt {
            return Err(ctx.err(
                ErrorCode::Internal,
                "хопы участка цикла из разных экосистем (формат Universal Router не совпал)",
            ));
        }
    }
    let path = route_calc::eval(ctx, legs, Side::Sell, input, Decimal::ONE)?;
    let slip = Decimal::ONE - ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let n = path.hops.len();
    let mut steps = Vec::with_capacity(n);
    let mut mins = Vec::with_capacity(n);
    for (j, h) in path.hops.iter().enumerate() {
        let keep = Decimal::ONE - h.usable.hook_pct / Decimal::ONE_HUNDRED;
        let min_out = num::to_units(h.pool_out * keep * slip, h.dec_out)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "минимум хопа цикла не считается"))?;
        if min_out.is_zero() {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                format!(
                    "{}: минимум хопа цикла 0 — обмен без минимума не отправляется",
                    h.entry.family.title
                ),
            ));
        }
        let from_wallet = j == 0;
        let amount_in = if from_wallet {
            input_units
        } else {
            CONTRACT_BALANCE
        };
        steps.push(Step {
            entry: h.entry.clone(),
            order: Order {
                token_in: h.token_in,
                token_out: h.token_out,
                amount_in,
                min_out,
            },
            from_wallet,
            to_wallet: j + 1 == n,
        });
        mins.push(min_out);
    }
    Ok((steps, mins))
}

/// Собирает бандл цикла. Принимает: ноги цикла в порядке исполнения (base → … → base, из quote/cycle::legs_for_cycle),
/// пробный/рабочий вход в базовой монете, кошелёк, первый nonce, время блока (секунды), нужны ли все разрешения
/// (true — не ходить на узел: сборка/тесты; false — только недостающие, узел). Возвращает: бандл и учёт пыли.
/// Ошибки: NoRoute (пусто, хоп не исполним бандлом v1), QuoteMismatch (минимум 0), NotConfigured (адреса/срок),
/// BadResponse (числа), Internal (план, форматы), узел (all_approvals=false).
pub async fn build(
    ctx: &Ctx,
    legs: &[Leg],
    input: Decimal,
    wallet: Address,
    first_nonce: u64,
    now: u64,
    all_approvals: bool,
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
    for (router, seg) in &segments {
        let (steps, mins) = segment_steps(ctx, seg, input_units)?;
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
