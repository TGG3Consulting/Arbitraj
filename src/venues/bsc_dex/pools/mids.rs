//! Файл: venues/bsc_dex/pools/mids.rs
//! Что делает: промежуточные монеты (решение 52) — переключатель dex.intermediate_enabled и список адресов
//! dex.intermediate_tokens [bsc_dex]; у каждой — тикер для затрат: тикер биржи по адресу (монета из подписки ядра)
//! или тикер нативной монеты на бирже (dex.native_ticker), если адрес — её обёртка: dex.native_wrapped (решение 270;
//! не задана — WETH() любого роутера V2, как раньше); ядро пересчитывает WBNB как BNB 1:1. WETH() роутера, не равный
//! dex.native_wrapped (чужая обёртка), тикер не получает. dex.native_ticker не задан — обёртка не используется (журнал).
//! Тикер неизвестен — монета не используется (ядро не пересчитает её затраты в USDT), причина — в журнал.
//! Решение 276: dex.intermediate_dex_priced = да — такая монета (и чужая обёртка, 270 дополнение 2) используется с
//! подписью с цепи (pools/mid_dex.rs), её затраты — «в цене», цена — пулом с USDT; цену BNB 1:1 она не получает.
//! При флаге = да тикер нативной монеты получает только адрес dex.native_wrapped; WETH() роутеров нативными не
//! считаются (не задана — нативной обёртки нет). Флаг «нет»/не задан — как до решения 276.
//! Промежуточная монета с любым налогом (покупки, продажи, перевода) в маршруте не используется — решение техлида
//! (Issue #7), проверка — quote/via_pools.rs.
//! Список найденных промежуточных монет хранится в памяти (Mids) — котировка и сделка читают его.
//! Решение 239: pinned — копия списка на момент снимка котировки (venues/bsc_dex/frozen.rs).
//! Кто вызывает: pools/market.rs (при подписке), quote/*, exec/*.
//! От чего зависит: ctx.rs, tokens.rs, node/call.rs, node/abi_dex.rs, pools/families.rs.
//! Решение 270 — деление WETH() на нативную и чужую: pools/wrapped.rs.
//! Настройки: dex.intermediate_enabled, dex.intermediate_tokens, dex.intermediate_dex_priced, dex.native_ticker, dex.native_wrapped [bsc_dex] (решение
//! 205: известная обёртка, если WETH() роутеров не прочитан); dex.router [вид пула V2] (WETH()).
//!
//! WETH() — IUniswapV2Router01.sol#L5 (v2-periphery ed249913), IPancakeRouter01.sol#L5 (pancake-swap-periphery
//! d769a6d1); «WBNB 1:1 к BNB по контракту обёртки» — ответ ядра в PR #25.

use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::abi_dex::IV2Router;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::families::{self, Proto};
use super::{mid_dex, wrapped};

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

    /// Решение 239: копия списка на этот момент (снимок котировки, frozen.rs) — своя память, пересборка подписки её
    /// не меняет. Возвращает: Mids. Ошибок нет.
    pub fn pinned(&self) -> Mids {
        Mids(Arc::new(RwLock::new(
            self.0.read().unwrap_or_else(|p| p.into_inner()).clone(),
        )))
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
        return Err(ctx.not_set(
            "dex.intermediate_tokens",
            "dex.intermediate_enabled = да, а список dex.intermediate_tokens пуст",
        ));
    }
    // Решение 205: обёртка — WETH() роутеров V2 и dex.native_wrapped (решение 62). У части форков (mdex_bsc, julswap,
    // журнал 06.11: «WETH() роутера не прочитан: execution reverted») WETH() нет — без настройки обёртка терялась бы,
    // и промежуточная WBNB выпадала, если остальные роутеры не ответили.
    // Решение 270: нативной считается только dex.native_wrapped; чужой WETH() роутера (fstswap) тикера BNB не получает.
    let configured = ctx.addr("dex.native_wrapped").ok();
    // Решение 276: необязательная; не задана — как «нет», поведение прежнее.
    let dex_priced = ctx.flag("dex.intermediate_dex_priced").unwrap_or(false);
    // Решение 270 (дополнение 2, ревью): при флаге = да нативная — только dex.native_wrapped; любой другой WETH()
    // (и все, если она не задана) тикер нативной монеты не получает и идёт веткой решения 276 (подпись с цепи).
    let (wrapped, foreign) = wrapped::natives(router_weths(ctx).await, configured, dex_priced);
    let native = ctx.text("dex.native_ticker");
    let mut out: Vec<Mid> = Vec::new();
    for address in list {
        let ticker = match tokens.ticker_of(address) {
            Some(t) => Ok(t),
            None if wrapped.contains(&address) => native.clone().map_err(|e| e.message),
            // Решение 276 (и 270, дополнение — чужая обёртка): подпись с цепи, цена — пулом с USDT.
            None if dex_priced => {
                let taken: Vec<String> = out.iter().map(|m| m.ticker.clone()).collect();
                Ok(mid_dex::label(ctx, tokens, address, &taken).await)
            }
            None => Err(
                if let Some((title, _)) = foreign.iter().find(|(_, a)| *a == address) {
                    wrapped::foreign_fact(title, address)
                } else {
                    "биржа не знает этот адрес и это не обёртка нативной монеты (WETH() роутера V2, dex.native_wrapped)"
                        .to_string()
                },
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

/// WETH() роутеров включённых видов V2 (DEX, адрес). Сбой вида — в журнал, вид пропускается. Делить на нативную и
/// чужую — wrapped::split (решение 270). Нужна и «Проверить подключение» (connection_extra.rs, решение 62).
pub async fn router_weths(ctx: &Ctx) -> Vec<(String, Address)> {
    let mut out = Vec::new();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.proto == Proto::V2)
    {
        if families::classify(ctx, f.id).is_err() {
            continue;
        }
        let res = match ctx.dex_addr("dex.router", f.id) {
            Ok(r) => call::view(ctx, r, &IV2Router::WETHCall {}, "latest").await,
            Err(e) => Err(e),
        };
        match res {
            Ok(a) => out.push((f.title.to_string(), a)),
            Err(e) => {
                // Решение 205: у роутера нет WETH() (форк без этой функции) — не помеха маршруту: обёртка известна из
                // других роутеров и dex.native_wrapped (read). Предупреждение — только если настройка не задана.
                let (level, tail) = match ctx.addr("dex.native_wrapped") {
                    Ok(_) => (Level::Info, "обёртка берётся из dex.native_wrapped"),
                    Err(_) => (
                        Level::Warn,
                        "dex.native_wrapped не задана — обёртка только из других роутеров",
                    ),
                };
                let msg = format!(
                    "{}: WETH() роутера не прочитан: {} — {tail}",
                    f.title, e.message
                );
                ctx.log(level, "dex.intermediate", msg, None).await;
            }
        }
    }
    out
}
