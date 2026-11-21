//! Файл: venues/bsc_dex/pools/crypto_variant.rs
//! Что делает: выбор версии формул crypto-пула Curve NG по MATH() пула (решение 453): у фабрики Twocrypto-NG Arbitrum
//! две реализации математики (v2.0.0 и v2.1.0, deployments.yaml `math_v2_0_0` / `math`), формулы get_y / newton_D у них
//! отличаются пределами assert'ов (math/crypto_lim2.rs). Пул зовёт свою математику (MATH() — immutable пула), поэтому
//! версия определяется адресом его математики, а не version() пула.
//! Ревью 453: адрес, заданный сразу в обоих списках, — отказ пула с причиной (версия формул неоднозначна), а не молчаливый
//! выбор v2.0.0; списки из настроек читаются одной функцией (lists): незаданная dex.curve_crypto_math_v2_0_0 — пустой
//! список (так задумано, docs/SETTINGS_CASES.md), ошибка разбора любой из двух — отказ с причиной.
//! Кто вызывает: pools/load_crypto.rs (state), pools/verify_crypto.rs (plan, decide).
//! От чего зависит: core/types (CryptoMath), ctx.rs (dex_list), alloy-primitives.
//! Настройки: dex.curve_crypto_math, dex.curve_crypto_math_v2_0_0 [DEX] (имена — здесь).

use alloy_primitives::Address;

use crate::core::types::CryptoMath;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

/// Настройка адресов математики Twocrypto-NG v2.1.0 и Tricrypto-NG (решение 442).
pub const MATH: &str = "dex.curve_crypto_math";

/// Настройка адресов математики Twocrypto-NG v2.0.0 (решение 453).
pub const MATH_V200: &str = "dex.curve_crypto_math_v2_0_0";

/// Что делает: версия формул по MATH() пула (решение 453). Принимает: число монет, MATH() пула, адреса математики
/// v2.0.0 (dex.curve_crypto_math_v2_0_0), адреса dex.curve_crypto_math. Возвращает: версию (2 монеты: v2.0.0 или
/// v2.1.0; 3 — Tricrypto-NG) или текст причины: адрес в обоих списках (неоднозначно), v2.0.0 у трёх монет, адреса нет
/// ни в одном списке (формула не сверена).
pub fn variant(
    n: usize,
    math: Address,
    v200: &[Address],
    ng: &[Address],
) -> Result<CryptoMath, String> {
    let (in_v200, in_ng) = (v200.contains(&math), ng.contains(&math));
    if in_v200 && in_ng {
        return Err(format!(
            "MATH() пула {math:#x} задан сразу в {MATH} и {MATH_V200} — версия формул неоднозначна, пул не берётся \
             (уберите адрес из одного списка)"
        ));
    }
    if in_v200 {
        if n == 2 {
            return Ok(CryptoMath::TwoNg200);
        }
        return Err(format!(
            "MATH() пула {math:#x} — из {MATH_V200} (математика Twocrypto-NG v2.0.0), а у пула {n} монеты — формула не \
             сверена"
        ));
    }
    if in_ng {
        return Ok(if n == 2 {
            CryptoMath::TwoNg210
        } else {
            CryptoMath::TriNg
        });
    }
    Err(format!(
        "MATH() пула {math:#x} нет в {MATH} / {MATH_V200} — формула не сверена"
    ))
}

/// Что делает: адреса математики из настроек DEX — (dex.curve_crypto_math_v2_0_0, dex.curve_crypto_math).
/// dex.curve_crypto_math_v2_0_0 необязательная (docs/SETTINGS_CASES.md: не задана — пулы с математикой v2.0.0 не
/// берутся) — не задана = пустой список; dex.curve_crypto_math — обязательная. Принимает: окружение, id DEX.
/// Ошибки: NotConfigured — не задана dex.curve_crypto_math; Internal — список любой из двух хранится не списком или
/// его элемент не адрес (ошибка разбора не гасится: пул не берётся с причиной).
pub fn lists(ctx: &Ctx, id: &str) -> GatewayResult<(Vec<Address>, Vec<Address>)> {
    let v200 = match ctx.dex_list(MATH_V200, id) {
        Ok(v) => v,
        Err(e) if e.code == ErrorCode::NotConfigured => Vec::new(),
        Err(e) => return Err(e),
    };
    let ng = ctx.dex_list(MATH, id)?;
    Ok((
        parse(ctx, MATH_V200, id, &v200)?,
        parse(ctx, MATH, id, &ng)?,
    ))
}

/// Строки списка → адреса. Принимает: окружение, имя настройки, id DEX, строки. Ошибки: Internal — элемент не адрес.
fn parse(ctx: &Ctx, name: &str, id: &str, list: &[String]) -> GatewayResult<Vec<Address>> {
    let mut out = Vec::with_capacity(list.len());
    for s in list {
        let Ok(a) = s.parse::<Address>() else {
            let why = format!("{name} [dex:{id}]: «{s}» — не адрес");
            return Err(ctx.err(ErrorCode::Internal, why));
        };
        out.push(a);
    }
    Ok(out)
}
