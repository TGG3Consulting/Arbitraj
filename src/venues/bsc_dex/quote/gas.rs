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
//! Кто вызывает: quote/mod.rs, exec/txs.rs, exec/bundle_send.rs, wallet/transfer.rs, frozen.rs (pinned).
//! От чего зависит: node/rpc.rs, node/heads.rs, num.rs, send/gas_mode.rs, quote/gas_base.rs.
//! Настройки: dex.gas_mode, send.gas_price_markup_pct (legacy), send.priority_fee_gwei и send.max_fee_margin_pct
//! (eip1559_l1) [площадка DEX]; узел — bsc.node_http_url.
//!
//! eth_gasPrice → wei — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/fee_market.yaml ;
//! «цену газа брать eth_gasPrice + запас из настроек, без зашитого минимума» — research/flow/bsc_dex.md, раздел 5.

use std::sync::{Arc, Mutex};

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::super::num;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::tx_1559::Fees;
use super::gas_base;

/// Цена газа с кэшем на блок. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct GasPrice {
    /// (блок, модель газа, цена узла: eth_gasPrice у legacy, baseFeePerGas у eip1559_l1).
    cache: Arc<Mutex<Option<(u64, GasMode, U256)>>>,
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

    /// Решение 352: цена газа в памяти — (блок, цена без надбавки); None — ещё не запрашивалась. Котировка на голове с
    /// этим номером берёт ровно её (hit в wei). Ошибок нет.
    pub fn known(&self) -> Option<(u64, U256)> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|(b, _, p)| (b, p))
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
        if mode != GasMode::Eip1559L1 {
            let why = format!(
                "цены типа 2 — только при dex.gas_mode = eip1559_l1 (сейчас {})",
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
            let (_, mode, p) = p.ok_or_else(|| {
                ctx.err(
                    ErrorCode::StaleData,
                    "цены газа на момент снимка в памяти не было — газ по снимку не посчитать",
                )
            })?;
            return Ok((mode, p));
        }
        let mode = gas_mode::read(ctx)?;
        let head = heads.fresh(ctx)?;
        let hit = || match *self.cache.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((b, m, p)) if b == head.number && m == mode => Some(p),
            _ => None,
        };
        // Последняя известная цена любого блока (той же модели) — чтобы расчёт не ждал узел (решение 89).
        let last = || match *self.cache.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((_, m, p)) if m == mode => Some(p),
            _ => None,
        };
        let base = match hit() {
            Some(p) => p,
            None => match self.flight.try_lock() {
                // Ведущий: на этом блоке кэша нет и никто не тянет — обновляем с узла.
                Ok(_one) => match hit() {
                    Some(p) => p,
                    None => self.fetch(ctx, head.number, mode).await?,
                },
                // Кто-то уже тянет (обычно фоновый прогрев): берём последнюю известную цену, не блокируемся на узле.
                Err(_) => match last() {
                    Some(p) => p,
                    // Холодный старт: ничего нет — один раз дожидаемся того запроса.
                    None => {
                        let _one = self.flight.lock().await;
                        match hit() {
                            Some(p) => p,
                            None => self.fetch(ctx, head.number, mode).await?,
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
        if mode == GasMode::Eip1559L1 {
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

    /// Обновляет кэш цены газа с узла: legacy — eth_gasPrice; eip1559_l1 — baseFeePerGas блока (gas_base.rs).
    /// Держится под flight — один запрос на всех. Принимает: окружение, номер блока, модель. Возвращает: цену в wei.
    /// Ошибки: узел.
    async fn fetch(&self, ctx: &Ctx, block: u64, mode: GasMode) -> GatewayResult<U256> {
        let p: U256 = match mode {
            GasMode::Legacy => rpc::call(ctx, "eth_gasPrice", json!([])).await?,
            GasMode::Eip1559L1 => gas_base::fetch(ctx, block).await?,
        };
        *self.cache.lock().unwrap_or_else(|x| x.into_inner()) = Some((block, mode, p));
        Ok(p)
    }
}
