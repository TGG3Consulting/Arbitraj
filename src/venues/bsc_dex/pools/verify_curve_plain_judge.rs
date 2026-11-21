//! Файл: venues/bsc_dex/pools/verify_curve_plain_judge.rs
//! Что делает: решение 472 (доделка) — итог проверки пула Curve StableSwap 2021 / метапула фабрики по ответам узла
//! (без сети): монеты пула coins(k); метка нативного ETH 0xEeee… (Plain*ETH) — обёртка dex.native_wrapped
//! (stable_native.rs, как у PancakeSwap Stable); обе монеты пары среди монет пула; у метапула — базовый пул фабрики
//! (get_base_pool) равен coins(1) метапула (LP-токен 2pool и fraxbp — сам пул) и запоминается для чтения
//! (load_curve_meta.rs remember); пул — multi_pair::adopt (решение 295). Вынесено из verify_curve_plain.rs (ориентир
//! 200 строк).
//! Кто вызывает: pools/verify_curve_plain.rs.
//! От чего зависит: ctx.rs, load_curve.rs (coins_of), stable_native.rs, verify_woo.rs (found), multi_pair.rs,
//! load_curve_meta.rs, abi_curve_plain.rs, load_curve_oracle.rs (решение 481: remember оракула пула).
//! Настройки: dex.native_wrapped (через stable_native.rs).
//!
//! Источники: curvefi/curve-factory f389123 Plain2ETH.vy L132-L135 (coins[0] = 0xEeee…); FactorySidechains.vy fb61207
//! get_base_pool L180-L188; MetaUSD.vy f389123 L623 (курс монеты 1 — get_virtual_price базового пула).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_curve_plain::ICurvePlainFactory;
use super::load_curve::coins_of;
use super::load_curve_meta;
use super::load_curve_oracle;
use super::multi_pair;
use super::stable_native;
use super::verify::{Candidate, Verdict};
use super::verify_woo;

/// Базовый пул метапула из ответа get_base_pool фабрики. Возвращает: адрес; Err(None) — откат или ноль (не метапул
/// фабрики); Err(Some) — сбой узла / ответ не по ABI. Ошибок иных нет.
pub fn base_of(answer: Option<call::Piece>) -> Result<Address, Option<String>> {
    let bytes: Bytes = match answer {
        Some(Ok(Ok(b))) => b,
        Some(Ok(Err(_))) => return Err(None),
        Some(Err(node)) => return Err(Some(node)),
        None => return Err(Some("нет ответа get_base_pool фабрики".into())),
    };
    let base = ICurvePlainFactory::get_base_poolCall::abi_decode_returns(&bytes)
        .map_err(|e| Some(format!("ответ get_base_pool не по ABI: {e}")))?;
    if base == Address::ZERO {
        return Err(None);
    }
    Ok(base)
}

/// Итог по ответам coins(k). Принимает: окружение, кандидата, пул, ответы coins(0..MAX_COINS), базовый пул (Some —
/// метапул), оракул курса (Some — пул с курсом оракула, решение 481: монет две, coins(0) — метка ETH; оракул
/// запоминается для чтения). Возвращает: Taken или Rejected с причиной. Ошибок нет.
pub fn judge(
    ctx: &Ctx,
    c: &Candidate,
    pool: Address,
    answers: &[Result<Bytes, String>],
    base: Option<Address>,
    oracle: Option<Address>,
) -> Verdict {
    let coins = match coins_of(answers) {
        Ok(v) => v,
        Err(w) => return Verdict::Rejected(w),
    };
    if coins.len() < 2 {
        return Verdict::Rejected(format!("у пула Curve {pool:#x} меньше двух монет"));
    }
    if let Some(b) = base {
        if coins.len() != 2 || coins[1] != b {
            return Verdict::Rejected(format!(
                "метапул Curve {pool:#x}: базовый пул фабрики {b:#x} не равен coins(1) — курс LP не прочитать"
            ));
        }
        load_curve_meta::remember(ctx.place.id, pool, b);
    }
    if let Some(o) = oracle {
        // Исходник ETH/wstETH: `assert _coins[0] == 0xEeee…` (__init__ L144), N_COINS = 2.
        if coins.len() != 2 || coins[0] != stable_native::NATIVE {
            return Verdict::Rejected(format!(
                "пул Curve с оракулом {pool:#x}: монет не две или coins(0) — не метка ETH 0xEeee… (не этот исходник)"
            ));
        }
        load_curve_oracle::remember(ctx.place.id, pool, o);
    }
    let mapped = match stable_native::pair_coins(&coins, stable_native::wrapped(ctx)) {
        Ok(v) => v,
        Err(why) => return Verdict::Rejected(format!("пул Curve {pool:#x}: {why}")),
    };
    if !mapped.contains(&c.coin) || !mapped.contains(&c.quote) {
        return Verdict::Rejected(format!(
            "среди монет пула Curve {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        ));
    }
    let found = verify_woo::found(c, PoolRef::Contract(pool));
    Verdict::Taken(Box::new(multi_pair::adopt(ctx, found, &mapped)))
}
