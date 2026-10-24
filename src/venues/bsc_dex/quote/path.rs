//! Файл: venues/bsc_dex/quote/path.rs
//! Что делает: путь обмена — один пул («USDT → монета») или два через промежуточную монету («USDT → промежуточная →
//! монета», решение 52) — в порядке исполнения: что входит в каждый пул, что выходит, что получено после крючка;
//! итог пути (монет, USDT). Один вид для прямого режима, долей умного режима, затрат по шагам и сделки.
//! Кто вызывает: quote/direct.rs, quote/via.rs, quote/smart.rs, quote/costs.rs, quote/steps.rs, exec/*.
//! От чего зависит: quote/direct.rs (Candidate), quote/select.rs (Usable), math/swap_cl.rs (Swapped), num.rs.
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{RouteHop, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::num;
use super::super::pools::registry::Entry;
use super::direct::Candidate;
use super::select::Usable;

/// Один пул пути.
#[derive(Debug, Clone)]
pub struct Hop {
    pub entry: Entry,
    pub usable: Usable,
    /// Обмен в пуле по локальной математике (минимальные единицы).
    pub swapped: Swapped,
    pub token_in: Address,
    pub token_out: Address,
    /// Сколько дошло до пула (в токене входа) и вышло из пула (в токене выхода).
    pub pool_in: Decimal,
    pub pool_out: Decimal,
    /// decimals токенов входа и выхода.
    pub dec_in: u8,
    pub dec_out: u8,
}

impl Hop {
    /// Шаг маршрута для ядра (вид пула, пул). Ошибок нет.
    pub fn route_hop(&self) -> RouteHop {
        RouteHop {
            family: self.entry.family.id.to_string(),
            kind: self.entry.pool.kind,
            pool: self.entry.pool.reference.clone(),
        }
    }
}

/// Путь: пулы в порядке исполнения и итог.
#[derive(Debug, Clone)]
pub struct Path {
    pub hops: Vec<Hop>,
    /// Монет получим (покупка) или продадим (продажа).
    pub coins: Decimal,
    /// USDT отдадим (покупка) или получим (продажа).
    pub usdt: Decimal,
}

impl Path {
    /// Путь из одного пула (прямой расчёт quote/direct.rs). Принимает: расчёт, сторону, (decimals монеты, USDT-стороны).
    /// Ошибки: BadResponse — суммы не помещаются в Decimal.
    pub fn one(
        ctx: &Ctx,
        c: Candidate,
        side: Side,
        (coin_dec, usdt_dec): (u8, u8),
    ) -> GatewayResult<Self> {
        let e = &c.entry;
        let (tin, tout, din, dout) = match side {
            Side::Buy => (e.usdt, e.coin, usdt_dec, coin_dec),
            Side::Sell => (e.coin, e.usdt, coin_dec, usdt_dec),
        };
        let hop = hop(
            ctx,
            c.entry.clone(),
            c.usable,
            c.swapped,
            (tin, tout),
            (din, dout),
        )?;
        Ok(Self {
            hops: vec![hop],
            coins: c.coins,
            usdt: c.usdt,
        })
    }

    /// Ожидаемый выход пути: монеты (покупка) или USDT (продажа). Ошибок нет.
    pub fn expected_out(&self, side: Side) -> Decimal {
        match side {
            Side::Buy => self.coins,
            Side::Sell => self.usdt,
        }
    }

    /// Единиц газа на весь путь (сумма газа обменов в каждом пуле, измеренного пробой). Ошибок нет.
    pub fn gas_units(&self) -> u64 {
        self.hops.iter().map(|h| h.usable.gas).sum()
    }
}

/// Пул пути по расчёту обмена. Принимает: пул, допуск, обмен, (вход, выход), (decimals входа, выхода).
/// Ошибки: BadResponse — суммы не помещаются в Decimal.
pub fn hop(
    ctx: &Ctx,
    entry: Entry,
    usable: Usable,
    swapped: Swapped,
    (token_in, token_out): (Address, Address),
    (din, dout): (u8, u8),
) -> GatewayResult<Hop> {
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма обмена не переводится в число",
        )
    };
    Ok(Hop {
        pool_in: num::to_decimal(swapped.amount_in, din).ok_or_else(bad)?,
        pool_out: num::to_decimal(swapped.amount_out, dout).ok_or_else(bad)?,
        entry,
        usable,
        swapped,
        token_in,
        token_out,
        dec_in: din,
        dec_out: dout,
    })
}
