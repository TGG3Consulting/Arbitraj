//! Файл: venues/bsc_dex/pools/load_balancer_v3_buffer.rs
//! Что делает: решение 474 — часть пачки чтения пула Balancer V3 на блоке: буферы ERC4626 хранилища для токенов пула
//! (boosted-пулы: токен пула — обёртка ERC4626, монета пары — её базовый токен). Хранилище: areBuffersPaused,
//! getMinimumWrapAmount; по каждому токену пула: getERC4626BufferAsset (0 — буфера нет), getBufferBalance; у самого
//! токена: asset() (хранилище при обмене требует asset() = базовый токен буфера — Vault.sol L1149-L1150),
//! previewDeposit / previewRedeem опорной суммы (dex.bv3_buffer_preview_raw). Разбор — по токену пула V3Buffer или None.
//! Читается каждым блоком вместе с пулом (буфер меняют обмены других, курс обёртки — время): пачка того же блока.
//! Кто вызывает: pools/load_balancer_v3.rs (calls, decode, preview_ref), тесты.
//! От чего зависит: ctx.rs, abi_balancer_v3_buffer.rs, load_balancer_v3_fixed.rs (Fixed), core/types (V3Buffer).
//! Настройки: dex.bv3_buffer_preview_raw [DEX вида balancer_v3] (не задана — буферы не читаются, обмена через буфер нет).
//!
//! Источник: шапка abi_balancer_v3_buffer.rs; Vault.sol (20241204-v3-vault) L1139-L1187 (erc4626BufferWrapOrUnwrap:
//! буфер инициализирован, asset() обёртки = базовый токен буфера, буферы не на паузе, наименьшая сумма).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::pool_vault::V3Buffer;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::abi_balancer_v3_buffer::{IBalancerV3Buffer as B, IErc4626 as W};
use super::load_balancer_v3_fixed::Fixed;

/// Имя настройки опорной суммы превью.
pub const SETTING: &str = "dex.bv3_buffer_preview_raw";

/// Вызовов на токен пула (порядок — calls).
const PER_TOKEN: usize = 5;

/// Опорная сумма превью из настроек DEX. Принимает: окружение, id DEX. Возвращает: None — не задана (буферы не
/// читаются); Some — сумма. Ошибки: Internal — не целое десятичное число или 0 (пул не читается с причиной).
pub fn preview_ref(ctx: &Ctx, id: &str) -> GatewayResult<Option<U256>> {
    let text = match ctx.dex_text(SETTING, id) {
        Ok(t) => t,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(None),
        Err(e) => return Err(e),
    };
    match U256::from_str(text.trim()) {
        Ok(v) if !v.is_zero() && !text.trim().starts_with("0x") => Ok(Some(v)),
        _ => Err(ctx.err(
            ErrorCode::Internal,
            format!("{SETTING} [dex:{id}] = «{text}» — нужно целое десятичное число больше 0"),
        )),
    }
}

/// Сколько вызовов буферов в пачке (0 — опорная сумма не задана). Принимает: неизменяемое пула. Ошибок нет.
pub fn count(fx: &Fixed) -> usize {
    match fx.buffer {
        Some(_) => 2 + PER_TOKEN * fx.tokens.len(),
        None => 0,
    }
}

/// Вызовы буферов (в конце пачки чтения пула). Принимает: хранилище, неизменяемое (fx.buffer — опорная сумма).
/// Возвращает: пусто, если опорная сумма не задана. Ошибок нет.
pub fn calls(vault: Address, fx: &Fixed) -> Vec<(Address, Bytes)> {
    let Some(r) = fx.buffer else {
        return Vec::new();
    };
    let c = |to: Address, d: Vec<u8>| (to, Bytes::from(d));
    let mut l = vec![
        c(vault, B::areBuffersPausedCall {}.abi_encode()),
        c(vault, B::getMinimumWrapAmountCall {}.abi_encode()),
    ];
    for t in &fx.tokens {
        let w = *t;
        l.push(c(
            vault,
            B::getERC4626BufferAssetCall { wrappedToken: w }.abi_encode(),
        ));
        l.push(c(
            vault,
            B::getBufferBalanceCall { wrappedToken: w }.abi_encode(),
        ));
        l.push(c(w, W::assetCall {}.abi_encode()));
        l.push(c(w, W::previewDepositCall { assets: r }.abi_encode()));
        l.push(c(w, W::previewRedeemCall { shares: r }.abi_encode()));
    }
    l
}

/// Разбор ответов буферов (без сети). Принимает: неизменяемое, ответы ровно count(fx) вызовов по порядку calls.
/// Возвращает: по токену пула — буфер или None (буфера нет, asset() обёртки не равен базовому токену буфера, превью
/// откатилось или 0 — обмен через буфер невозможен / не считается). Пусто — опорная сумма не задана. Ошибки (текст):
/// откат или не по ABI у areBuffersPaused / getMinimumWrapAmount / getERC4626BufferAsset / getBufferBalance хранилища.
pub fn decode(fx: &Fixed, got: &[Result<Bytes, String>]) -> Result<Vec<Option<V3Buffer>>, String> {
    let Some(reference) = fx.buffer else {
        return Ok(Vec::new());
    };
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(w)) => Err(format!("буфер: вызов {i} откатился: {w}")),
        None => Err(format!("буфер: нет ответа на вызов {i}")),
    };
    let dec = |e: alloy::sol_types::Error| format!("буфер: ответ не по ABI: {e}");
    let paused = B::areBuffersPausedCall::abi_decode_returns(ok(0)?).map_err(dec)?;
    let min_wrap = B::getMinimumWrapAmountCall::abi_decode_returns(ok(1)?).map_err(dec)?;
    let mut out = Vec::with_capacity(fx.tokens.len());
    for (k, wrapped) in fx.tokens.iter().enumerate() {
        let at = 2 + PER_TOKEN * k;
        let underlying = B::getERC4626BufferAssetCall::abi_decode_returns(ok(at)?).map_err(dec)?;
        let bal = B::getBufferBalanceCall::abi_decode_returns(ok(at + 1)?).map_err(dec)?;
        // Обёртка — чужой контракт: откат или ответ не по ABI — у этого токена обмена через буфер нет.
        let soft = |i: usize| got.get(i).and_then(|r| r.as_ref().ok());
        let asset = soft(at + 2).and_then(|b| W::assetCall::abi_decode_returns(b).ok());
        let deposit = soft(at + 3).and_then(|b| W::previewDepositCall::abi_decode_returns(b).ok());
        let redeem = soft(at + 4).and_then(|b| W::previewRedeemCall::abi_decode_returns(b).ok());
        let buf = match (asset, deposit, redeem) {
            (Some(a), Some(dep), Some(red))
                if !underlying.is_zero() && a == underlying && !dep.is_zero() && !red.is_zero() =>
            {
                Some(V3Buffer {
                    wrapped: *wrapped,
                    underlying,
                    min_wrap,
                    reference,
                    deposit: dep,
                    redeem: red,
                    underlying_bal: bal.underlyingBalanceRaw,
                    wrapped_bal: bal.wrappedBalanceRaw,
                    paused,
                })
            }
            _ => None,
        };
        out.push(buf);
    }
    Ok(out)
}
