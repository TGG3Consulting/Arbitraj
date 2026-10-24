//! Файл: venues/bsc_dex/pools/mids.rs
//! Что делает: промежуточные монеты (решение 52) — переключатель dex.intermediate_enabled и список адресов
//! dex.intermediate_tokens [bsc_dex]; у каждой — тикер для затрат: тикер биржи по адресу (монета из подписки ядра)
//! или тикер нативной монеты на бирже (dex.native_ticker), если адрес — её обёртка (WETH() роутера V2 своей DEX; ядро
//! пересчитывает WBNB как BNB 1:1). Настройка не задана — обёртка не используется (причина в журнал).
//! Тикер неизвестен — монета не используется (ядро не пересчитает её затраты в USDT), причина — в журнал.
//! Промежуточная монета с любым налогом (покупки, продажи, перевода) в маршруте не используется — решение техлида
//! (Issue #7), проверка — quote/via_pools.rs.
//! Список найденных промежуточных монет хранится в памяти (Mids) — котировка и сделка читают его.
//! Кто вызывает: pools/market.rs (при подписке), quote/*, exec/*.
//! От чего зависит: ctx.rs, tokens.rs, node/call.rs, node/abi_dex.rs, pools/families.rs, pools/discover.rs.
//! Настройки: dex.intermediate_enabled, dex.intermediate_tokens, dex.native_ticker [bsc_dex]; dex.router [вид пула V2] (WETH()).
//!
//! WETH() — IUniswapV2Router01.sol#L5 (v2-periphery ed249913), IPancakeRouter01.sol#L5 (pancake-swap-periphery
//! d769a6d1); «WBNB 1:1 к BNB по контракту обёртки» — ответ ядра в PR #25.

use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::abi_dex::IV2Router;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::discover;
use super::families::{FAMILIES, Proto};

/// Промежуточная монета: адрес и тикер для затрат.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mid {
    pub address: Address,
    pub ticker: String,
}

/// Найденные промежуточные монеты. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Mids(Arc<RwLock<Vec<Mid>>>);

impl Mids {
    /// Заменяет список. Ошибок нет.
    pub fn set(&self, v: Vec<Mid>) {
        *self.0.write().unwrap_or_else(|p| p.into_inner()) = v;
    }

    /// Список, если промежуточные монеты сейчас включены (dex.intermediate_enabled = да; выключение действует сразу,
    /// новые адреса — после пересборки подписки). Ошибок нет: не задано или выключено — пусто.
    pub fn active(&self, ctx: &Ctx) -> Vec<Mid> {
        match ctx.flag("dex.intermediate_enabled") {
            Ok(true) => self.0.read().unwrap_or_else(|p| p.into_inner()).clone(),
            _ => Vec::new(),
        }
    }

    /// Тикер промежуточной монеты по адресу. None — не промежуточная.
    pub fn ticker(&self, a: Address) -> Option<String> {
        self.0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|m| m.address == a)
            .map(|m| m.ticker.clone())
    }
}

/// Читает настройки и находит тикеры. Принимает: окружение, токены подписки.
/// Возвращает: промежуточные монеты (выключено — пусто). Ошибки: NotConfigured — переключатель не задан или включён
/// с пустым списком (значений по умолчанию нет).
pub async fn read(ctx: &Ctx, tokens: &Tokens) -> GatewayResult<Vec<Mid>> {
    if !ctx.flag("dex.intermediate_enabled")? {
        return Ok(Vec::new());
    }
    let list = ctx
        .settings
        .addresses("dex.intermediate_tokens", ctx.at())
        .map_err(|e| ctx.tag(e))?;
    if list.is_empty() {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "dex.intermediate_enabled = да, а список dex.intermediate_tokens пуст",
        ));
    }
    let wrapped = wrapped_native(ctx).await;
    let native = ctx.text("dex.native_ticker");
    let mut out = Vec::new();
    for address in list {
        let ticker = match tokens.ticker_of(address) {
            Some(t) => Ok(t),
            None if wrapped.contains(&address) => native.clone().map_err(|e| e.message),
            None => Err(
                "биржа не знает этот адрес и это не обёртка нативной монеты (WETH() роутера V2)"
                    .to_string(),
            ),
        };
        match ticker {
            Ok(ticker) => out.push(Mid { address, ticker }),
            Err(why) => {
                let msg = format!(
                    "Промежуточная монета {address}: {why} — затраты не пересчитать, не используется"
                );
                ctx.log(Level::Warn, "dex.intermediate", msg, None).await;
            }
        }
    }
    Ok(out)
}

/// Адреса обёртки нативной монеты: WETH() роутеров включённых видов V2. Сбой вида — в журнал, вид пропускается.
async fn wrapped_native(ctx: &Ctx) -> Vec<Address> {
    let mut out = Vec::new();
    for f in FAMILIES.iter().filter(|f| f.proto == Proto::V2) {
        if !discover::enabled(ctx, f).await {
            continue;
        }
        let res = match ctx.dex_addr("dex.router", f.id) {
            Ok(r) => call::view(ctx, r, &IV2Router::WETHCall {}, "latest").await,
            Err(e) => Err(e),
        };
        match res {
            Ok(a) => out.push(a),
            Err(e) => {
                let msg = format!("{}: WETH() роутера не прочитан: {}", f.title, e.message);
                ctx.log(Level::Warn, "dex.intermediate", msg, None).await;
            }
        }
    }
    out
}
