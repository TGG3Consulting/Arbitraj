//! Файл: venues/bsc_dex/pools/created_crypto.rs
//! Что делает: событие создания crypto-пула Curve фабрикой (решение 453, задача 6 решения 442) → кандидаты на проверку.
//! Twocrypto-NG (TwocryptoPoolDeployed) и Tricrypto-NG (TricryptoPoolDeployed) несут адрес пула и монеты
//! (неиндексированные поля — разбор данных лога): кандидат — пул и каждая монета пула при встречном токене среди монет
//! (у Tricrypto — до двух пар). Фабрика crypto v1 (CryptoPoolDeployed) адреса пула не несёт — только монеты: пулы пары
//! ищутся по реестру фабрики find_pool_for_coins(c0, c1, i), i < dex.pair_lookup_max, на блоке события (нулевой адрес —
//! пулов больше нет); кандидаты — все пулы пары (проверка и подключение отбрасывают уже известные). Дальше — обычная
//! проверка (verify.rs) и подключение (created_log.rs).
//! Кто вызывает: pools/created_log.rs (sources — topic0, one — кандидаты).
//! От чего зависит: ctx.rs, node/call.rs (try_many), abi_crypto.rs (find_pool_for_coins), abi_crypto_v1.rs
//! (CryptoPoolDeployed), factory_calls.rs (lookup_max), families.rs, verify.rs (Candidate).
//! Настройки: dex.pair_lookup_max [DEX] (через factory_calls::lookup_max).
//!
//! Источники: curvefi/twocrypto-ng 89c8dfb CurveTwocryptoFactory.vy событие L18-L30, log L230-L243 (у фабрики Arbitrum
//! развёрнут тот же текст — d612fde, git diff пуст); curvefi/tricrypto-ng ecaa816 CurveL2TricryptoFactory.vy событие
//! L18-L31, log L239-L253; curvefi/curve-factory-crypto 7ed1011 Factory.vy событие L38-L51, markets L117, запись
//! markets[key][length] = pool L219-L222, find_pool_for_coins L335-L345.

use alloy::rpc::types::Log;
use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_crypto::ICryptoFactory;
use super::abi_crypto_v1::ICryptoFactoryV1;
use super::factory_calls::lookup_max;
use super::families::{Family, Proto};
use super::verify::Candidate;

// События и функции контрактов Curve несут больше 7 полей — clippy::too_many_arguments на сгенерированных
// конструкторах alloy::sol! (код макроса, не наш).
#[allow(clippy::too_many_arguments)]
mod sol {
    alloy::sol! {
        interface INgFactory {
            event TwocryptoPoolDeployed(
                address pool,
                string name,
                string symbol,
                address[2] coins,
                address math,
                bytes32 salt,
                uint256[2] precisions,
                uint256 packed_A_gamma,
                uint256 packed_fee_params,
                uint256 packed_rebalancing_params,
                uint256 packed_prices,
                address deployer
            );

            event TricryptoPoolDeployed(
                address pool,
                string name,
                string symbol,
                address weth,
                address[3] coins,
                address math,
                bytes32 salt,
                uint256 packed_precisions,
                uint256 packed_A_gamma,
                uint256 packed_fee_params,
                uint256 packed_rebalancing_params,
                uint256 packed_prices,
                address deployer
            );
        }
    }
}
pub use sol::*;

/// topic0 события создания пула фабрики вида. None — у вида нет события фабрики (tricrypto 2021). Ошибок нет.
pub fn topic(proto: Proto) -> Option<B256> {
    match proto {
        Proto::CurveTwocrypto => Some(INgFactory::TwocryptoPoolDeployed::SIGNATURE_HASH),
        Proto::CurveTricrypto => Some(INgFactory::TricryptoPoolDeployed::SIGNATURE_HASH),
        Proto::CurveCryptoV1 => Some(ICryptoFactoryV1::CryptoPoolDeployed::SIGNATURE_HASH),
        _ => None,
    }
}

/// Что делает: кандидаты из события создания пула. Принимает: окружение, вид, лог, встречный токен наблюдателя.
/// Возвращает: кандидатов (пусто — встречного токена нет среди монет пула). Ошибки: лог не по ABI, нет
/// dex.pair_lookup_max / dex.factory (v1), узел (v1).
pub async fn candidates(
    ctx: &Ctx,
    f: &'static Family,
    log: &Log,
    quote: Address,
) -> Result<Vec<Candidate>, String> {
    let data = &log.inner.data;
    let (pools, coins): (Vec<Address>, Vec<Address>) = match f.proto {
        Proto::CurveTwocrypto => {
            let e = INgFactory::TwocryptoPoolDeployed::decode_log_data(data)
                .map_err(|e| format!("TwocryptoPoolDeployed не по ABI: {e}"))?;
            (vec![e.pool], e.coins.to_vec())
        }
        Proto::CurveTricrypto => {
            let e = INgFactory::TricryptoPoolDeployed::decode_log_data(data)
                .map_err(|e| format!("TricryptoPoolDeployed не по ABI: {e}"))?;
            (vec![e.pool], e.coins.to_vec())
        }
        Proto::CurveCryptoV1 => {
            let e = ICryptoFactoryV1::CryptoPoolDeployed::decode_log_data(data)
                .map_err(|e| format!("CryptoPoolDeployed не по ABI: {e}"))?;
            let coins = e.coins.to_vec();
            if !coins.contains(&quote) {
                return Ok(Vec::new());
            }
            (lookup(ctx, f, log, coins[0], coins[1]).await?, coins)
        }
        _ => return Ok(Vec::new()),
    };
    Ok(pairs(f, &pools, &coins, quote))
}

/// Кандидаты (пул × монета пула, кроме встречного) при встречном токене среди монет. Ошибок нет.
pub fn pairs(
    f: &'static Family,
    pools: &[Address],
    coins: &[Address],
    quote: Address,
) -> Vec<Candidate> {
    if !coins.contains(&quote) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for pool in pools {
        for coin in coins.iter().filter(|c| **c != quote) {
            out.push(Candidate {
                family: f,
                pool: format!("{pool:#x}"),
                coin: *coin,
                quote,
            });
        }
    }
    out
}

/// Пулы пары у фабрики crypto v1 на блоке события: find_pool_for_coins(a, b, i), i < dex.pair_lookup_max, до первого
/// нулевого адреса. Ошибки: настройки, узел, ответ не по ABI.
async fn lookup(
    ctx: &Ctx,
    f: &Family,
    log: &Log,
    a: Address,
    b: Address,
) -> Result<Vec<Address>, String> {
    let factory = ctx.dex_addr("dex.factory", f.id).map_err(|e| e.message)?;
    let n = lookup_max(ctx, f.id)?;
    let calls: Vec<(Address, Bytes)> = (0..n)
        .map(|i| {
            let q = ICryptoFactory::find_pool_for_coinsCall {
                from: a,
                to: b,
                i: U256::from(i),
            };
            (factory, q.abi_encode().into())
        })
        .collect();
    let tag = log
        .block_number
        .map(call::block_tag)
        .unwrap_or_else(|| "latest".into());
    let got = call::try_many(ctx, &calls, &tag)
        .await
        .map_err(|e| e.message)?;
    let mut out = Vec::new();
    for r in got {
        let bytes = r.map_err(|w| format!("find_pool_for_coins откатился: {w}"))?;
        let pool = ICryptoFactory::find_pool_for_coinsCall::abi_decode_returns(&bytes)
            .map_err(|e| format!("find_pool_for_coins не по ABI: {e}"))?;
        if pool.is_zero() {
            break;
        }
        out.push(pool);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;

    /// topic0 — подписи событий из исходников фабрик (строки — шапка).
    #[test]
    fn topics_match_source() {
        let two = concat!(
            "TwocryptoPoolDeployed(address,string,string,address[2],address,bytes32,uint256[2],",
            "uint256,uint256,uint256,uint256,address)"
        );
        let three = concat!(
            "TricryptoPoolDeployed(address,string,string,address,address[3],address,bytes32,",
            "uint256,uint256,uint256,uint256,uint256,address)"
        );
        assert_eq!(
            topic(Proto::CurveTwocrypto),
            Some(keccak256(two.as_bytes()))
        );
        assert_eq!(
            topic(Proto::CurveTricrypto),
            Some(keccak256(three.as_bytes()))
        );
        assert_eq!(topic(Proto::CurveTricryptoV1), None);
    }
}
