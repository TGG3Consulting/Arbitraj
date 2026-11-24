//! Единица отображения ERC-8056 на одном блоке. Используется вне поиска объёма.
//! Источник ABI: https://eips.ethereum.org/EIPS/eip-8056 и https://eips.ethereum.org/EIPS/eip-165.
//! decimals() проверяется в той же пачке: пустые ответы от адреса без кода не означают обычный ERC-20.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::OnceCell;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::Ctx;
use super::node::{abi::IERC20, jsonrpc, multicall, rpc};
use super::num;

alloy::sol! {
    function supportsInterface(bytes4 interfaceId) external view returns (bool);
    function uiMultiplier() external view returns (uint256);
}

fn probes(token: Address) -> [(Address, Bytes); 5] {
    let supports = |id: [u8; 4]| {
        supportsInterfaceCall {
            interfaceId: id.into(),
        }
        .abi_encode()
    };
    [
        IERC20::decimalsCall {}.abi_encode(),
        supports(uiMultiplierCall::SELECTOR),
        supports(supportsInterfaceCall::SELECTOR),
        supports([0xff; 4]),
        uiMultiplierCall {}.abi_encode(),
    ]
    .map(|data| (token, Bytes::from(data)))
}

type Sample = Arc<OnceCell<(u8, Option<Decimal>)>>;

#[derive(Default)]
struct Samples {
    height: u64,
    at_height: HashMap<Address, Sample>,
}

/// Общий для кошельков одного экземпляра DEX. На новом блоке старые результаты не переиспользуются.
#[derive(Clone, Default)]
pub(super) struct Units(Arc<Mutex<Samples>>);

impl Units {
    fn sample(&self, token: Address, block: u64) -> Sample {
        let mut saved = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if block < saved.height {
            return Sample::default();
        }
        if block > saved.height {
            saved.height = block;
            saved.at_height.clear();
        }
        saved.at_height.entry(token).or_default().clone()
    }

    pub(super) async fn read(
        &self,
        ctx: &Ctx,
        token: Address,
        block: u64,
    ) -> GatewayResult<(u8, Option<Decimal>)> {
        let sample = self.sample(token, block);
        sample
            .get_or_try_init(|| read(ctx, token, block))
            .await
            .copied()
    }
}

async fn read(ctx: &Ctx, token: Address, block: u64) -> GatewayResult<(u8, Option<Decimal>)> {
    let block = format!("0x{block:x}");
    let calls = probes(token);
    let results: Vec<Option<Bytes>> = if let Some(mc) = multicall::address(ctx) {
        multicall::aggregate(ctx, mc, &calls, &block)
            .await?
            .into_iter()
            .map(Result::ok)
            .collect()
    } else {
        // Без Multicall3 нет общего EVM-вызова. Сохраняем код ошибки каждого JSON-RPC ответа:
        // отсутствие функции может вызвать откат, а тайм-аут/отсутствующий блок требуют повтора.
        let params: Vec<_> = calls
            .iter()
            .map(|(to, data)| ("eth_call", json!([{ "to": to, "input": data }, block])))
            .collect();
        let mut results = Vec::with_capacity(calls.len());
        for item in rpc::batch(ctx, &params).await? {
            results.push(match item {
                Ok(raw) => Some(jsonrpc::decode::<Bytes>(&raw, rpc::SOURCE)?),
                Err(e) if e.code == ErrorCode::SwapReverted => None,
                Err(e) => return Err(e),
            });
        }
        results
    };
    decode(&results).map_err(|why| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Единица токена {token} на блоке {block}: {why}"),
        )
    })
}

fn word(bytes: &Option<Bytes>) -> Result<Option<U256>, &'static str> {
    let data: Option<&[u8]> = bytes.as_ref().map(|b| b.as_ref());
    match data {
        None | Some([]) => Ok(None),
        Some(b) if b.len() == 32 => Ok(Some(U256::from_be_slice(b))),
        _ => Err("ответ не является ABI-словом"),
    }
}

fn decode(results: &[Option<Bytes>]) -> Result<(u8, Option<Decimal>), &'static str> {
    let [decimals, supported, erc165, invalid, multiplier] = results else {
        return Err("неполная пачка ответов");
    };
    let decimals = word(decimals)?
        .and_then(|d| u8::try_from(d).ok())
        .ok_or("контракт не вернул decimals()")?;
    let supported = word(supported)?;
    let erc165 = word(erc165)?;
    let invalid = word(invalid)?;
    let multiplier = word(multiplier)?;
    let one = U256::from(1u8);
    let zero = U256::ZERO;
    if supported.unwrap_or(zero) == zero
        && erc165.unwrap_or(zero) <= one
        && invalid.unwrap_or(zero) == zero
        && multiplier.unwrap_or(zero) == zero
    {
        return Ok((decimals, None));
    }
    if supported != Some(one) || erc165 != Some(one) || invalid != Some(zero) {
        return Err("ответы ERC-165 не подтверждают интерфейс ERC-8056");
    }
    let raw = multiplier
        .filter(|m| *m > zero)
        .ok_or("нет положительного uiMultiplier()")?;
    // Для коэффициента округление до вместимости Decimal было бы другой единицей.
    let scale = num::to_decimal(raw, 18)
        .filter(|m| num::to_units(*m, 18) == Some(raw))
        .ok_or("uiMultiplier() не представим точно")?;
    Ok((decimals, Some(scale.normalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archived_units_at_entry() {
        #[derive(serde::Deserialize)]
        struct Recorded {
            calldata: Vec<Bytes>,
            tokens: std::collections::HashMap<Address, Vec<Option<Bytes>>>,
        }
        let saved: Recorded = serde_json::from_str(include_str!(
            "../../../tests/data/bsc_units_2026_11_24/answers.json"
        ))
        .unwrap();
        let cases = [
            (
                "0x3ee4df61bd4f867e349beae8bfe07bc31b4850fb",
                Some(Decimal::from(8)),
            ),
            ("0x55d398326f99059ff775485246999027b3197955", None),
            (
                "0x80f3d493ebce97e343c53d29a137942416b4ffc0",
                Some(Decimal::ONE),
            ),
        ];
        for (address, expected) in cases {
            let token: Address = address.parse().unwrap();
            assert_eq!(&probes(token).map(|(_, data)| data)[..], &saved.calldata);
            assert_eq!(decode(&saved.tokens[&token]), Ok((18, expected)));
        }
    }
}
