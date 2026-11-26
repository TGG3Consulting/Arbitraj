//! Файл: venues/bsc_dex/quote/gas.rs
//! Что делает: цена газа для расчёта и отправки — eth_gasPrice узла с надбавкой send.gas_price_markup_pct;
//! запрашивается не чаще одного раза на блок на всех (значение до прихода нового блока — из памяти). Решение 89: чтобы
//! расчёт НЕ ждал узел — если запрос уже в полёте (single-flight занят), возвращаем ПОСЛЕДНЮЮ известную цену любого
//! блока, не блокируясь; сам запрос обычно делает фоновый прогрев (quote/warm.rs) на каждый новый блок, а не расчёт.
//! Единственное исключение — холодный старт (кэш пуст): один раз дожидаемся первого запроса.
//! Минимум канала отправки (channel.min_gas_price_gwei) применяется при отправке (exec/txs.rs) и в котировке (quote/mod.rs).
//! Решение 237: pinned — цена газа, закреплённая на момент снимка котировки (venues/bsc_dex/frozen.rs): котировка по
//! снимку считает газ по цене момента решения, а не по цене блока, пришедшего позже.
//! Решение 435 (433-д): модель газа площадки dex.gas_mode (send/gas_mode.rs) читается на каждом запросе цены:
//! legacy — как было (eth_gasPrice + надбавка); eip1559_l1 — база блока головы (baseFeePerGas, quote/gas_base.rs) +
//! чаевые send.priority_fee_gwei, а для отправки — цены типа 2 (fees). Не задана — цены нет (NotConfigured), котировки
//! и отправки площадки нет. Кэш помнит модель: смена модели на лету — новый запрос к узлу.
//! Решение 436 (Б): при eip1559_l1 вместе с baseFee блока (одновременно) читается минимум цены газа сети —
//! ArbGasInfo.getMinimumGasPrice (node/arb_min_gas.rs, dex.arb_gas_info; не задана — не читается); сбой чтения —
//! цены газа на этом блоке нет. Минимум берёт канал «Секвенсор Arbitrum» (send/ready.rs). Отдельного опроса нет.
//! Кто вызывает: quote/mod.rs, exec/txs.rs, exec/bundle_send.rs, wallet/transfer.rs, frozen.rs (pinned).
//! От чего зависит: node/rpc.rs, node/heads.rs, num.rs, send/gas_mode.rs, quote/gas_base.rs, node/arb_min_gas.rs.
//! Настройки: dex.gas_mode, send.gas_price_markup_pct (legacy), send.priority_fee_gwei и send.max_fee_margin_pct
//! (eip1559_l1), dex.arb_gas_info (eip1559_l1, решение 436) [площадка DEX]; узел — bsc.node_http_url.
//!
//! eth_gasPrice → wei — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/fee_market.yaml ;
//! «цену газа брать eth_gasPrice + запас из настроек, без зашитого минимума» — research/flow/bsc_dex.md, раздел 5.

use std::sync::{Arc, Mutex};

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::arb_min_gas;
use super::super::node::heads::Heads;
use super::super::node::op_fees::{Oracle, Tariff};
use super::super::node::rpc;
use super::super::num;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::tx_1559::Fees;
use super::gas_base;

#[derive(Clone, Copy)]
struct Entry {
    block: u64,
    mode: GasMode,
    chain: Option<u64>,
    price: U256,
    tariff: Option<Tariff>,
}

/// Цена газа с кэшем на блок. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct GasPrice {
    /// (блок, модель газа, цена узла: eth_gasPrice у legacy, baseFeePerGas у eip1559_l1).
    cache: Arc<Mutex<Option<Entry>>>,
    /// Один запрос на блок на всех: одновременные промахи ждут первого (решение 61).
    flight: Arc<tokio::sync::Mutex<()>>,
    /// Решение 237: закреплена на момент снимка — узел не спрашивается, свежесть блока не проверяется.
    pinned: bool,
}

impl GasPrice {
    /// Решение 237: копия с ценой, закреплённой сейчас (последняя известная цена из памяти, без запроса к узлу; цены
    /// ещё нет — котировка по копии вернёт StaleData, живая цена не подставляется). Возвращает: GasPrice. Ошибок нет.
    pub fn pinned(&self) -> GasPrice {
        let now = *self.cache.lock().unwrap_or_else(|p| p.into_inner());
        GasPrice {
            cache: Arc::new(Mutex::new(now)),
            flight: Arc::default(),
            pinned: true,
        }
    }

    /// Для Base закрепляет базовую цену, L1 и оператора в одном снимке перед расчётом вариантов сделки.
    pub async fn for_quote(&self, ctx: &Ctx, heads: &Heads) -> GatewayResult<Self> {
        if !self.pinned && gas_mode::optional(ctx)? == Some(GasMode::OpStack) {
            self.base(ctx, heads).await?;
            return Ok(self.pinned());
        }
        Ok(self.clone())
    }

    /// Решение 352: цена газа в памяти — (блок, цена без надбавки); None — ещё не запрашивалась. Котировка на голове с
    /// этим номером берёт ровно её (hit в wei). Ошибок нет.
    pub fn known(&self) -> Option<(u64, U256)> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|e| (e.block, e.price))
    }

    /// Надбавка Base в wei для одного или нескольких разрешений/обменов. Снимок котировки не обращается к RPC.
    pub fn extra(&self, ctx: &Ctx, gas: u64, txs: usize) -> GatewayResult<U256> {
        if txs == 0 || gas_mode::optional(ctx)? != Some(GasMode::OpStack) {
            return Ok(U256::ZERO);
        }
        let oracle = Oracle::read(ctx)?;
        let cached = *self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let chain = Some(ctx.int("dex.evm_chain_id")?);
        let tariff = cached
            .filter(|entry| entry.chain == chain && entry.mode == GasMode::OpStack)
            .and_then(|e| e.tariff)
            .filter(|t| t.oracle == oracle)
            .ok_or_else(|| {
                ctx.err(
                    ErrorCode::StaleData,
                    "нет тарифа Base для выбранного предела размера транзакции",
                )
            })?;
        Ok(tariff.extra(gas, txs))
    }

    /// Цена газа в wei для плана: legacy — с надбавкой; eip1559_l1 — база + чаевые (закреплённая — решение 237: без
    /// узла и без проверки свежести блока).
    /// Ошибки: StaleData (нет свежего блока; у закреплённой — цены не было), NotConfigured (dex.gas_mode, надбавка,
    /// чаевые), узел.
    pub async fn wei(&self, ctx: &Ctx, heads: &Heads) -> GatewayResult<U256> {
        let (mode, base) = self.base(ctx, heads).await?;
        self.marked(ctx, mode, base)
    }

    /// Решение 435 (433-д): цены транзакции типа 2 (eip1559_l1) от той же базы, что wei. Принимает: окружение, головы.
    /// Возвращает: (цены, база блока). Ошибки: как wei; NotConfigured — модель площадки не eip1559_l1 или нет запаса.
    pub async fn fees(&self, ctx: &Ctx, heads: &Heads) -> GatewayResult<(Fees, U256)> {
        let (mode, base) = self.base(ctx, heads).await?;
        if mode == GasMode::Legacy {
            let why = format!(
                "цены типа 2 — только для модели EIP-1559 (сейчас {})",
                mode.as_str()
            );
            return Err(ctx.not_set("dex.gas_mode", why));
        }
        Ok((gas_base::fees(ctx, base)?, base))
    }

    /// Цена узла (без надбавки и чаевых) и модель газа — из кэша на блок или с узла (решения 89, 237).
    /// Ошибки: как wei.
    async fn base(&self, ctx: &Ctx, heads: &Heads) -> GatewayResult<(GasMode, U256)> {
        if self.pinned {
            let p = *self.cache.lock().unwrap_or_else(|p| p.into_inner());
            let entry = p.ok_or_else(|| {
                ctx.err(
                    ErrorCode::StaleData,
                    "цены газа на момент снимка в памяти не было — газ по снимку не посчитать",
                )
            })?;
            if entry.mode.strict_type2()
                && (gas_mode::read(ctx)? != entry.mode
                    || Some(ctx.int("dex.evm_chain_id")?) != entry.chain
                    || entry.tariff.map(|t| t.oracle) != Self::oracle(ctx, entry.mode)?)
            {
                return Err(ctx.err(
                    ErrorCode::StaleData,
                    "Изменилась сеть или модель закреплённой цены газа",
                ));
            }
            return Ok((entry.mode, entry.price));
        }
        let mode = gas_mode::read(ctx)?;
        let oracle = Self::oracle(ctx, mode)?;
        let chain = mode
            .strict_type2()
            .then(|| ctx.int("dex.evm_chain_id"))
            .transpose()?;
        let head = heads.fresh(ctx)?;
        let hit = || match *self.cache.lock().unwrap_or_else(|p| p.into_inner()) {
            Some(e)
                if e.block == head.number
                    && e.mode == mode
                    && e.chain == chain
                    && e.tariff.map(|t| t.oracle) == oracle =>
            {
                Some(e.price)
            }
            _ => None,
        };
        // Последняя известная цена любого блока (той же модели) — чтобы расчёт не ждал узел (решение 89).
        let last = || match *self.cache.lock().unwrap_or_else(|p| p.into_inner()) {
            Some(e)
                if e.mode == mode
                    && e.chain == chain
                    && e.tariff.map(|t| t.oracle) == oracle
                    && mode != GasMode::OpStack =>
            {
                Some(e.price)
            }
            _ => None,
        };
        let base = match hit() {
            Some(p) => p,
            None => match self.flight.try_lock() {
                // Ведущий: на этом блоке кэша нет и никто не тянет — обновляем с узла.
                Ok(_one) => match hit() {
                    Some(p) => p,
                    None => self.fetch(ctx, head.number, mode, chain, oracle).await?,
                },
                // Кто-то уже тянет (обычно фоновый прогрев): берём последнюю известную цену, не блокируемся на узле.
                Err(_) => match last() {
                    Some(p) => p,
                    // Холодный старт: ничего нет — один раз дожидаемся того запроса.
                    None => {
                        let _one = self.flight.lock().await;
                        match hit() {
                            Some(p) => p,
                            None => self.fetch(ctx, head.number, mode, chain, oracle).await?,
                        }
                    }
                },
            },
        };
        Ok((mode, base))
    }

    /// Цена для плана: legacy — с надбавкой send.gas_price_markup_pct; eip1559_l1 — база + чаевые (gas_base.rs).
    /// Принимает: окружение, модель, цену узла в wei. Возвращает: wei.
    /// Ошибки: NotConfigured (надбавка, чаевые), BadResponse (не считается).
    fn marked(&self, ctx: &Ctx, mode: GasMode, base: U256) -> GatewayResult<U256> {
        if mode != GasMode::Legacy {
            return gas_base::planned(ctx, base);
        }
        let markup = ctx.dec("send.gas_price_markup_pct")?;
        let bad = || {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Цена газа {base} wei не считается с надбавкой"),
            )
        };
        let v = num::to_decimal(base, 0).ok_or_else(bad)?
            * (Decimal::ONE + markup / Decimal::ONE_HUNDRED);
        num::to_units(v, 0).ok_or_else(bad)
    }

    fn oracle(ctx: &Ctx, mode: GasMode) -> GatewayResult<Option<Oracle>> {
        (mode == GasMode::OpStack)
            .then(|| Oracle::read(ctx))
            .transpose()
    }

    /// Обновляет кэш цены газа с узла: legacy — eth_gasPrice; eip1559_l1 — baseFeePerGas блока (gas_base.rs) и
    /// минимум цены газа сети (node/arb_min_gas.rs refresh, решение 436).
    /// Держится под flight — один запрос на всех. Принимает: окружение, номер блока, модель. Возвращает: цену в wei.
    /// Ошибки: узел.
    async fn fetch(
        &self,
        ctx: &Ctx,
        block: u64,
        mode: GasMode,
        chain: Option<u64>,
        oracle: Option<Oracle>,
    ) -> GatewayResult<U256> {
        let mut tariff = None;
        let p: U256 = match mode {
            GasMode::Legacy => rpc::call(ctx, "eth_gasPrice", json!([])).await?,
            GasMode::Ethereum => gas_base::fetch(ctx, block).await?,
            GasMode::OpStack => {
                let oracle = oracle.ok_or_else(|| {
                    ctx.not_set("dex.op_gas_price_oracle", "не задан GasPriceOracle Base")
                })?;
                let (price, t) =
                    tokio::try_join!(gas_base::fetch(ctx, block), oracle.tariff(ctx, block))?;
                tariff = Some(t);
                price
            }
            GasMode::Eip1559L1 => {
                // Решение 436 (Б): минимум цены газа сети (ArbGasInfo) — с узла, вместе с baseFee того же блока.
                let (p, ()) = tokio::try_join!(
                    gas_base::fetch(ctx, block),
                    arb_min_gas::refresh(ctx, block)
                )?;
                p
            }
        };
        if chain.is_some()
            && (gas_mode::read(ctx)? != mode
                || Some(ctx.int("dex.evm_chain_id")?) != chain
                || Self::oracle(ctx, mode)? != oracle)
        {
            return Err(ctx.err(
                ErrorCode::StaleData,
                "Сеть или оракул изменились при получении цены газа",
            ));
        }
        *self.cache.lock().unwrap_or_else(|x| x.into_inner()) = Some(Entry {
            chain,
            block,
            mode,
            price: p,
            tariff,
        });
        Ok(p)
    }
}
