//! Файл: venues/bsc_dex/pools/load_wombat_calls.rs
//! Что делает: решение 484 — вызовы чтения пула Wombat и разбор ответов без сети (тест на записи узла собирает то же
//! состояние из тех же ответов): ступень 1 — getTokens, ampFactor, haircutRate, startCovRatio, endCovRatio, paused
//! пула; ступень 2 — addressOfAsset(монета) по каждой монете; ступень 3 — у каждого актива underlyingToken,
//! underlyingTokenDecimals, pool, cash, liability и (динамический пул) getRelativePrice; при известных монетах и активах
//! (load_wombat_memo.rs) — одна пачка меняющихся величин (vary_calls / parse_vary). Сборка состояния — build
//! (load_wombat_build.rs, вынесено по размеру файла).
//! Кто вызывает: pools/load_wombat.rs; tests/arbitrum_wombat_live.rs.
//! От чего зависит: abi_wombat.rs, load_wombat_build.rs, load_wombat_memo.rs (Fixed).
//! Настройки: нет.
//!
//! Источник — шапка abi_wombat.rs (геттеры PoolV2 / HighCovRatioFeePoolV2 / Asset / DynamicAsset).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::abi_wombat::{IWombatAsset as A, IWombatPool as P};
pub use super::load_wombat_build::build;
use super::load_wombat_memo::Fixed;

/// Ответ вызова пачки: байты или текст отката.
type Ans = Result<Bytes, String>;

/// Ступень 1 — параметры пула.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub tokens: Vec<Address>,
    pub amp: U256,
    pub haircut: U256,
    pub start_cov: U256,
    pub end_cov: U256,
    pub paused: bool,
}

/// Ступень 3 — актив одной монеты.
#[derive(Debug, Clone, PartialEq)]
pub struct Side {
    pub cash: U256,
    pub liability: U256,
    pub decimals: u8,
    /// getRelativePrice() — только у динамического пула.
    pub price: Option<U256>,
}

/// Байты ответа или причина. Ошибок нет.
fn ok<'a>(got: &'a [Ans], i: usize, what: &str) -> Result<&'a Bytes, String> {
    match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("{what} откатился: {r}")),
        None => Err(format!("нет ответа {what}")),
    }
}

/// Разбор ответа по ABI вызова C. Ошибок нет (текст причины).
fn dec<C: SolCall>(got: &[Ans], i: usize, what: &str) -> Result<C::Return, String> {
    let b = ok(got, i, what)?;
    C::abi_decode_returns(b).map_err(|e| format!("{what} не по ABI: {e}"))
}

/// Вызовы ступени 1. Принимает: пул. Ошибок нет.
pub fn head_calls(pool: Address) -> Vec<(Address, Bytes)> {
    vec![
        (pool, P::getTokensCall {}.abi_encode().into()),
        (pool, P::ampFactorCall {}.abi_encode().into()),
        (pool, P::haircutRateCall {}.abi_encode().into()),
        (pool, P::startCovRatioCall {}.abi_encode().into()),
        (pool, P::endCovRatioCall {}.abi_encode().into()),
        (pool, P::pausedCall {}.abi_encode().into()),
    ]
}

/// Разбор ступени 1. Принимает: ответы head_calls. Возвращает: Head или причину (откат, не по ABI).
pub fn parse_head(got: &[Ans]) -> Result<Head, String> {
    Ok(Head {
        tokens: dec::<P::getTokensCall>(got, 0, "getTokens()")?,
        amp: dec::<P::ampFactorCall>(got, 1, "ampFactor()")?,
        haircut: dec::<P::haircutRateCall>(got, 2, "haircutRate()")?,
        start_cov: U256::from(dec::<P::startCovRatioCall>(got, 3, "startCovRatio()")?),
        end_cov: U256::from(dec::<P::endCovRatioCall>(got, 4, "endCovRatio()")?),
        paused: dec::<P::pausedCall>(got, 5, "paused()")?,
    })
}

/// Вызовы ступени 2 — addressOfAsset(монета). Ошибок нет.
pub fn asset_calls(pool: Address, tokens: &[Address]) -> Vec<(Address, Bytes)> {
    tokens
        .iter()
        .map(|t| (pool, P::addressOfAssetCall { token: *t }.abi_encode().into()))
        .collect()
}

/// Разбор ступени 2. Возвращает: адреса активов по порядку монет или причину.
pub fn parse_assets(got: &[Ans], tokens: &[Address]) -> Result<Vec<Address>, String> {
    (0..tokens.len())
        .map(|k| dec::<P::addressOfAssetCall>(got, k, &format!("addressOfAsset({})", tokens[k])))
        .collect()
}

/// Число вызовов ступени 3 на актив. Принимает: динамический ли пул. Ошибок нет.
pub fn per_asset(dynamic: bool) -> usize {
    if dynamic { 6 } else { 5 }
}

/// Вызовы ступени 3. Принимает: активы, динамический ли пул. Ошибок нет.
pub fn detail_calls(assets: &[Address], dynamic: bool) -> Vec<(Address, Bytes)> {
    let mut out = Vec::new();
    for a in assets {
        out.push((*a, A::underlyingTokenCall {}.abi_encode().into()));
        out.push((*a, A::underlyingTokenDecimalsCall {}.abi_encode().into()));
        out.push((*a, A::poolCall {}.abi_encode().into()));
        out.push((*a, A::cashCall {}.abi_encode().into()));
        out.push((*a, A::liabilityCall {}.abi_encode().into()));
        if dynamic {
            out.push((*a, A::getRelativePriceCall {}.abi_encode().into()));
        }
    }
    out
}

/// Разбор ступени 3 со сверкой: underlyingToken актива = монета, pool() актива = пул. Принимает: ответы detail_calls,
/// пул, монеты, динамический ли пул. Возвращает: стороны по порядку монет или причину.
pub fn parse_detail(
    got: &[Ans],
    pool: Address,
    tokens: &[Address],
    dynamic: bool,
) -> Result<Vec<Side>, String> {
    let n = per_asset(dynamic);
    let mut out = Vec::with_capacity(tokens.len());
    for (k, t) in tokens.iter().enumerate() {
        let at = k * n;
        let under = dec::<A::underlyingTokenCall>(got, at, "underlyingToken()")?;
        if under != *t {
            return Err(format!("актив монеты {t}: underlyingToken() = {under}"));
        }
        let owner = dec::<A::poolCall>(got, at + 2, "pool()")?;
        if owner != pool {
            return Err(format!("актив монеты {t}: pool() = {owner}, не этот пул"));
        }
        let price = if dynamic {
            Some(dec::<A::getRelativePriceCall>(got, at + 5, "getRelativePrice()")?)
        } else {
            None
        };
        let decimals = dec::<A::underlyingTokenDecimalsCall>(got, at + 1, "decimals")?;
        let cash = dec::<A::cashCall>(got, at + 3, "cash()")?.to::<u128>();
        let liability = dec::<A::liabilityCall>(got, at + 4, "liability()")?.to::<u128>();
        out.push(Side {
            cash: U256::from(cash),
            liability: U256::from(liability),
            decimals,
            price,
        });
    }
    Ok(out)
}

/// Решение 484 (ревью t253): вызовы чтения каждым блоком при известных монетах и активах (load_wombat_memo.rs) — одна
/// пачка: ampFactor, haircutRate, startCovRatio, endCovRatio, paused пула и у каждого актива cash, liability
/// (динамический пул — ещё getRelativePrice). Ошибок нет.
pub fn vary_calls(pool: Address, assets: &[Address], dynamic: bool) -> Vec<(Address, Bytes)> {
    let mut out = head_calls(pool).split_off(1);
    for a in assets {
        out.push((*a, A::cashCall {}.abi_encode().into()));
        out.push((*a, A::liabilityCall {}.abi_encode().into()));
        if dynamic {
            out.push((*a, A::getRelativePriceCall {}.abi_encode().into()));
        }
    }
    out
}

/// Разбор vary_calls. Принимает: ответы, неизменные части пула, динамический ли пул. Возвращает: (Head, стороны) или
/// причину (откат, не по ABI).
pub fn parse_vary(got: &[Ans], fixed: &Fixed, dynamic: bool) -> Result<(Head, Vec<Side>), String> {
    let head = Head {
        tokens: fixed.tokens.clone(),
        amp: dec::<P::ampFactorCall>(got, 0, "ampFactor()")?,
        haircut: dec::<P::haircutRateCall>(got, 1, "haircutRate()")?,
        start_cov: U256::from(dec::<P::startCovRatioCall>(got, 2, "startCovRatio()")?),
        end_cov: U256::from(dec::<P::endCovRatioCall>(got, 3, "endCovRatio()")?),
        paused: dec::<P::pausedCall>(got, 4, "paused()")?,
    };
    let n = if dynamic { 3 } else { 2 };
    let mut sides = Vec::with_capacity(fixed.assets.len());
    for (k, d) in fixed.decimals.iter().enumerate() {
        let at = 5 + k * n;
        let cash = dec::<A::cashCall>(got, at, "cash()")?.to::<u128>();
        let liability = dec::<A::liabilityCall>(got, at + 1, "liability()")?.to::<u128>();
        let price = if dynamic {
            Some(dec::<A::getRelativePriceCall>(got, at + 2, "getRelativePrice()")?)
        } else {
            None
        };
        sides.push(Side {
            cash: U256::from(cash),
            liability: U256::from(liability),
            decimals: *d,
            price,
        });
    }
    Ok((head, sides))
}
