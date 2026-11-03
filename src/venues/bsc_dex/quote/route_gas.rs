//! Файл: venues/bsc_dex/quote/route_gas.rs
//! Что делает: газ пути графа в USDT для ВЫБОРА маршрута (решение 122, часть R1, как `gasInclude` у KyberSwap):
//! газ пути = сумма газа обмена каждого его пула (замер пробным обменом своей стороны пула — quote/select.rs Usable.gas,
//! check/state.rs) × текущая цена газа (quote/gas.rs — та же цена, что у котировки и отправки); BNB газа (= WBNB 1:1)
//! переводится в USDT продажей этой суммы в лучший достоверный пул «WBNB — USDT» реестра нашей математикой
//! (quote/rate.rs). Сплит (quote/graph_split.rs) вычитает газ из прироста новой доли: доля/путь берётся по ЧИСТОМУ
//! выходу. Сама затрата газа в котировке по-прежнему считается отдельно (quote/steps.rs) — здесь только выбор.
//! Кто вызывает: quote/graph_split.rs (через quote/graph_route.rs).
//! От чего зависит: quote/rate.rs, quote/route_calc.rs (Leg), pools/registry.rs, num.rs, ctx.rs.
//! Настройки: dex.route_gas_aware [bsc_dex] (да/нет, необязательная: не задана или «нет» — сплит без учёта газа, как
//! до решения 122); dex.native_wrapped [bsc_dex] — адрес WBNB для перевода газа в USDT.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::Registry;
use super::rate;
use super::route_calc::Leg;

/// Включён ли учёт газа при выборе маршрута (dex.route_gas_aware = да). Не задана / «нет» / не читается — нет
/// (настройка необязательная, поведение как до решения 122). Ошибок нет.
pub fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("dex.route_gas_aware").ok() == Some(true)
}

/// Перевод газа в USDT для одной котировки: цена газа, USDT, WBNB, decimals USDT.
pub struct GasUsdt<'a> {
    pub ctx: &'a Ctx,
    pub reg: &'a Registry,
    pub usdt: Address,
    pub usdt_dec: u8,
    pub wbnb: Address,
    pub gas_wei: U256,
}

impl<'a> GasUsdt<'a> {
    /// Готовит перевод. Принимает: окружение, реестр, USDT и его decimals, цену газа (wei).
    /// Ошибки: NotConfigured — dex.native_wrapped (WBNB) не задан: газ в USDT не перевести.
    pub fn new(
        ctx: &'a Ctx,
        reg: &'a Registry,
        (usdt, usdt_dec): (Address, u8),
        gas_wei: U256,
    ) -> GatewayResult<Self> {
        let wbnb = ctx.addr("dex.native_wrapped").map_err(|_| {
            ctx.not_set(
                "dex.native_wrapped",
                "dex.route_gas_aware = да: нужен адрес WBNB (dex.native_wrapped), чтобы перевести газ пути в USDT",
            )
        })?;
        Ok(Self {
            ctx,
            reg,
            usdt,
            usdt_dec,
            wbnb,
            gas_wei,
        })
    }

    /// Газ пути в USDT. Принимает: ноги пути (у каждой — газ обмена своей стороны пула). Возвращает: USDT.
    /// Ошибки: NoRoute — нет достоверного пула «WBNB — USDT» (rate.rs); BadResponse — сумма не помещается в число.
    pub fn path(&self, legs: &[Leg]) -> GatewayResult<Decimal> {
        let units: u64 = legs.iter().map(|l| l.usable.gas).sum();
        let wei = self.gas_wei.saturating_mul(U256::from(units));
        if wei.is_zero() {
            return Ok(Decimal::ZERO);
        }
        // BNB газа = WBNB 1:1 (18 знаков, num::BNB_DECIMALS): продаём эту сумму WBNB в пул «WBNB — USDT».
        let got = rate::to_usdt(self.ctx, self.reg, self.usdt, self.wbnb, wei)?;
        num::to_decimal(got, self.usdt_dec).ok_or_else(|| {
            self.ctx.err(
                ErrorCode::BadResponse,
                format!("газ пути {got} (единиц USDT) не переводится в число"),
            )
        })
    }
}
