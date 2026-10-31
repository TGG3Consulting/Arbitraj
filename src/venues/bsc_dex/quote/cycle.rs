//! Файл: venues/bsc_dex/quote/cycle.rs
//! Что делает: расчёт ОДНОГО замкнутого цикла DEX ↔ DEX (решение 85) как сигнала — без исполнения. Цикл: старт = финиш =
//! базовая монета кошелька, через пулы разных DEX обратно в неё же. Переиспользует общий расчёт маршрута
//! (quote/route_calc.rs): цикл считается «вперёд с точным входом» (как сторона продажи — каждый пул принимает точный
//! вход), вход маршрута = базовая монета, выход = она же. Налога монеты на стороне нет — через налоговые токены цикл
//! не строим (quote/cycle_scan.rs фильтрует, как мост через промежуточную quote/via.rs; поэтому tax_keep = 1), так что
//! «налог токена на каждом свопе» (решение 85) структурно равен нулю. В выходе уже учтены комиссии всех пулов, влияние
//! крючков и проскальзывание на каждом шаге (математика пулов). Отдельно считаются газ BSC свопов и чаевые канала (BNB),
//! чистая прибыль = (выход − вход) базовой монеты − эти затраты, переведённые в базовую монету (quote/cycle_scan.rs).
//! Транзитных затрат (вывод, перевод, депозит, время в пути, риск движения цены за перевод) у цикла НЕТ — перевода нет,
//! цикл в одном блоке (решение 85).
//! Исполнимость (решение 84/85, фаза сигнала): цикл «исполним», только если ВСЕ его хопы через tradable-пулы; иначе —
//! сигнал, но не исполнимый (экзотические семейства сейчас tradable = false). Сам бандл-исполнитель цикла — следующая
//! фаза; авто-сделки по циклам этот код НЕ запускает.
//! Кто вызывает: quote/cycle_scan.rs (перебор циклов по базовым монетам); тесты.
//! От чего зависит: quote/route_calc.rs (Leg, eval), quote/path.rs (Path, Hop), quote/steps.rs (fee_pct),
//! quote/graph_legs.rs (hop_side), quote/route_paths.rs (RoutePath), quote/select.rs (Usable), pools/registry.rs,
//! pools/families.rs (tradable), num.rs.
//! Настройки: нет своих; через route_calc/steps — dex.v2_fee_pct [вид пула V2].

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{PoolKind, PoolRef, Side};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::num;
use super::graph_legs::hop_side;
use super::path::Path;
use super::route_calc::{self, Leg};
use super::route_paths::RoutePath;
use super::steps;

/// Один хоп цикла для сигнала/карточки (решение 74): что через какой пул и на каких условиях меняется.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleHop {
    /// Номер шага по порядку, с 1.
    pub index: u32,
    /// Вид пула у адаптера (id семейства DEX).
    pub family: String,
    pub kind: PoolKind,
    pub pool: PoolRef,
    pub token_in: Address,
    pub token_out: Address,
    /// Комиссия пула на этом шаге, % (уже в выходе).
    pub fee_pct: Decimal,
    /// Влияние крючка B/C на этом шаге, % (уже в выходе).
    pub hook_pct: Decimal,
    /// Газ обмена в этом пуле (единиц).
    pub gas_units: u64,
}

/// Сигнал одного цикла DEX ↔ DEX (решение 85): вход, выход, прибыль, маршрут, исполнимость. Чистая прибыль посчитана,
/// только если газ/чаевые (BNB) удалось перевести в базовую монету (quote/cycle_scan.rs); иначе net_profit = None и
/// видна валовая прибыль с затратами в BNB отдельно (ничего не скрыто; «не торгуем вслепую» — показываем, что считать
/// нечем).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleSignal {
    /// Базовая монета (старт = финиш цикла).
    pub base: Address,
    /// Тикер базовой монеты (для карточки).
    pub base_ticker: String,
    /// Вложено базовой монеты (вход цикла).
    pub input: Decimal,
    /// Получено базовой монеты (выход цикла; комиссии пулов, крючки, проскальзывание уже внутри).
    pub output: Decimal,
    /// Валовая прибыль базовой монеты = выход − вход (комиссии/крючки/проскальзывание уже учтены).
    pub gross_profit: Decimal,
    /// Единиц газа на все свопы цикла.
    pub gas_units: u64,
    /// Газ свопов цикла, BNB.
    pub gas_bnb: Decimal,
    /// Чаевые канала отправки, BNB (решение 41; из настроек, 0 — не заданы).
    pub tip_bnb: Decimal,
    /// Газ разрешений (approve) в этот расчёт НЕ входит (зависит от состояния кошелька, добавляется в фазе исполнения).
    pub bnb_cost: Decimal,
    /// Затраты BNB, переведённые в базовую монету (None — перевести нечем: нет пула базовая ↔ WBNB).
    pub bnb_cost_in_base: Option<Decimal>,
    /// Чистая прибыль базовой монеты = gross_profit − bnb_cost_in_base (None — газ/чаевые не переведены в базовую).
    pub net_profit: Option<Decimal>,
    /// Маршрут цикла по шагам.
    pub hops: Vec<CycleHop>,
    /// Цикл исполним как есть (все хопы через tradable-пулы) — решение 84/85. Исполнитель цикла — следующая фаза;
    /// авто-сделки этот сигнал НЕ запускает.
    pub executable: bool,
    /// Почему не исполним (для карточки). None — исполним.
    pub exec_note: Option<String>,
    /// Блок, по состоянию которого посчитан цикл (минимальный среди пулов маршрута).
    pub block: u64,
}

/// Ноги цикла в порядке исполнения (base → … → base), допуск каждого пула — по стороне свопа на этом хопе
/// (hop_side, как в quote/graph_legs.rs), decimals — из `dec`. Все хопы идут «вперёд», как записаны в цикле
/// (переворота стороны, как у продажи в graph_legs, здесь нет — цикл уже ориентирован от base к base).
/// Ошибка (текст) — пул не допущен или нет decimals: такой цикл вызывающий пропускает.
pub fn legs_for_cycle(
    q: &super::Quoting,
    path: &RoutePath,
    usdt_dec: u8,
    dec: &HashMap<Address, u8>,
) -> Result<Vec<Leg>, String> {
    let mut legs = Vec::with_capacity(path.steps.len());
    for s in &path.steps {
        let entry = &s.edge.entry;
        let hs = hop_side(entry, s.token_in).ok_or("ребро не соединяет токены хопа")?;
        let usable = super::select::usable(q, entry, hs, usdt_dec)
            .map_err(|w| format!("{} {}: {w}", entry.family.title, entry.ticker))?;
        let dec_in = *dec.get(&s.token_in).ok_or("нет decimals токена входа")?;
        let dec_out = *dec.get(&s.token_out).ok_or("нет decimals токена выхода")?;
        legs.push(Leg {
            entry: entry.clone(),
            usable,
            token_in: s.token_in,
            token_out: s.token_out,
            dec_in,
            dec_out,
        });
    }
    Ok(legs)
}

/// Расчёт цикла: «вперёд с точным входом» (та же математика, что у продажи в route_calc), вход = `input` базовой
/// монеты, tax_keep = 1 (через налоговые токены цикл не строим — quote/cycle_scan.rs). Возвращает путь: hops в порядке
/// исполнения, coins = вход базовой, usdt = выход базовой. Ошибки: NoRoute (пусто, шаг не исполним целиком), BadResponse.
pub fn eval(ctx: &Ctx, legs: &[Leg], input: Decimal) -> GatewayResult<Path> {
    route_calc::eval(ctx, legs, Side::Sell, input, Decimal::ONE)
}

/// Исполним ли цикл как есть (решение 84/85, фаза сигнала): ВСЕ хопы — через tradable-пулы. Иначе — причина (первое
/// не-tradable семейство): это сигнал, но авто-сделкой (бандлом) пока не исполняется. Ошибок нет.
pub fn executable(legs: &[Leg]) -> Result<(), String> {
    if legs.is_empty() {
        return Err("пустой цикл".to_string());
    }
    if let Some(l) = legs.iter().find(|l| !l.entry.family.tradable) {
        return Err(format!(
            "цикл проходит через {} без своего пути исполнения — это сигнал, авто-сделкой не исполняется",
            l.entry.family.id
        ));
    }
    Ok(())
}

/// Сборка сигнала цикла из рассчитанного пути. Принимает: окружение, базовую монету и её тикер, путь (eval), ноги
/// (для исполнимости), цену газа (wei), чаевые канала (BNB), курс BNB → базовая (base за 1 BNB; None — перевести
/// нечем). Чистая прибыль = валовая − (газ + чаевые)·курс. Ошибки: BadResponse (комиссия пула, числа газа).
#[allow(clippy::too_many_arguments)]
pub fn build_signal(
    ctx: &Ctx,
    base: Address,
    base_ticker: &str,
    path: &Path,
    legs: &[Leg],
    gas_wei: U256,
    tip_bnb: Decimal,
    bnb_in_base: Option<Decimal>,
) -> GatewayResult<CycleSignal> {
    let mut hops = Vec::with_capacity(path.hops.len());
    let mut block = u64::MAX;
    let mut gas_units = 0u64;
    for (i, h) in path.hops.iter().enumerate() {
        block = block.min(h.entry.pool.block);
        gas_units += h.usable.gas;
        hops.push(CycleHop {
            index: i as u32 + 1,
            family: h.entry.family.id.to_string(),
            kind: h.entry.pool.kind,
            pool: h.entry.pool.reference.clone(),
            token_in: h.token_in,
            token_out: h.token_out,
            fee_pct: steps::fee_pct(ctx, &h.entry, h.token_in)?,
            hook_pct: h.usable.hook_pct,
            gas_units: h.usable.gas,
        });
    }
    let gas_bnb = num::to_decimal(
        gas_wei.saturating_mul(U256::from(gas_units)),
        num::BNB_DECIMALS,
    )
    .ok_or_else(|| {
        ctx.err(
            crate::gateway::errors::ErrorCode::BadResponse,
            "газ цикла не помещается в Decimal",
        )
    })?;
    let bnb_cost = gas_bnb + tip_bnb;
    let bnb_cost_in_base = bnb_in_base.map(|r| bnb_cost * r);
    let gross_profit = path.usdt - path.coins;
    let net_profit = bnb_cost_in_base.map(|c| gross_profit - c);
    let exec = executable(legs);
    Ok(CycleSignal {
        base,
        base_ticker: base_ticker.to_string(),
        input: path.coins,
        output: path.usdt,
        gross_profit,
        gas_units,
        gas_bnb,
        tip_bnb,
        bnb_cost,
        bnb_cost_in_base,
        net_profit,
        hops,
        executable: exec.is_ok(),
        exec_note: exec.err(),
        block,
    })
}

/// Проходит ли сигнал порог прибыли (решение 85: торгуем, только если прибыль после всех затрат выше порога).
/// Сравнивается чистая прибыль (net_profit); не посчитана (газ не переведён в базовую) — порог не пройден.
/// Принимает: порог в базовой монете. Ошибок нет.
impl CycleSignal {
    pub fn profitable(&self, min_profit_base: Decimal) -> bool {
        self.net_profit.is_some_and(|p| p >= min_profit_base)
    }
}
