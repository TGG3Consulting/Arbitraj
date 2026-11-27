//! Фоновая проверка ERC-20. Наблюдение двух balanceOf выбирает конкретный
//! адрес хранения и ключ. Код getter доказывает разрядность целого без смещения.
use super::super::{
    check::{probe_calls, simulate},
    ctx::Ctx,
    node::rpc,
    quote::white_state::Head,
};
use super::{
    presell_getter::{Code, Read},
    presell_warm::Cache,
};
use alloy_primitives::{Address, B256, Bytes, U256, b256, keccak256};
use serde_json::json;
mod binding;
mod observed;
use binding::{binding, original, original_for};
mod denial;
pub(super) use denial::{Account, Denial, Negative};
use observed::Reads;

pub(super) const IMPL_SLOT: B256 =
    b256!("360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc");
pub(super) const ADMIN_SLOT: B256 =
    b256!("b53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103");
pub(super) const BEACON_SLOT: B256 =
    b256!("a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BalanceField {
    pub(super) storage: Address,
    pub(super) key: B256,
    pub(super) max: U256,
}
impl BalanceField {
    /// Используется только в той же симуляции и на том же кошельке, что и проверка.
    pub(super) fn credit(self, amount: U256) -> Option<simulate::Override> {
        (amount <= self.max).then(|| simulate::Override {
            address: self.storage,
            code: Bytes::new(),
            state: vec![(self.key, B256::from(amount.to_be_bytes::<32>()))],
            diff: true,
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Identity {
    runtime: B256,
    target: Option<(Address, B256)>,
    implementation_slot: bool,
    admin: Option<B256>,
}
pub(super) struct Proof {
    pub(super) identity: Identity,
    pub(super) field: BalanceField,
    pub(super) balance: U256,
}

fn balance_word(balance: &[u8], raw: B256) -> Result<U256, String> {
    if balance != raw.as_slice() {
        return Err("слово баланса отличается от balanceOf".into());
    }
    Ok(U256::from_be_bytes(raw.0))
}
pub(super) async fn verify(
    ctx: &Ctx,
    t: Address,
    wallet: Address,
    h: Head,
    generation: u64,
    cache: &Cache,
    previous: Option<(Identity, BalanceField)>,
) -> Result<Proof, Denial> {
    if let Some((previous, field)) = previous {
        let reads = Reads {
            other: previous.target.map(|(a, _)| a),
            implementation: previous.implementation_slot,
            admin: previous.admin.is_some(),
        };
        if let Some((identity, _, _)) = binding(ctx, t, h, reads, generation, cache).await?
            && identity == previous
        {
            let balance = original(ctx, t, wallet, field, h).await?;
            return Ok(Proof {
                identity,
                field,
                balance,
            });
        }
    }
    if wallet.is_zero() {
        return Err("нет адреса своего кошелька".into());
    }
    let control = Address::ZERO;
    let call = probe_calls::balance(wallet, t, wallet);
    let other = probe_calls::balance(wallet, t, control);
    let (a, b) = tokio::try_join!(
        observed::call(ctx, &call, h),
        observed::call(ctx, &other, h)
    )?;
    let (fields, reads) =
        observed::compare(&a, &b, t).map_err(|why| Denial::observed(why, &a, &b, t))?;
    let (identity, token, implementation) = binding(ctx, t, h, reads, generation, cache)
        .await?
        .ok_or_else(|| "ERC1967: указатель не совпадает со свежим списком чтения".to_string())?;
    let mut verified = Vec::with_capacity(2);
    for (holder, location) in [wallet, control].into_iter().zip(fields) {
        let max = Read {
            token: &token,
            implementation: implementation.as_ref(),
            admin: identity.admin,
            caller: wallet,
            holder,
            key: location.key,
            storage: location.storage,
        }
        .mask()
        .ok_or_else(|| {
            Denial::observed(
                "не подтверждён возврат uintN из balanceOf".into(),
                &a,
                &b,
                t,
            )
        })?;
        verified.push(location.bounded(max));
    }
    let field = verified[0];
    if field.max != verified[1].max {
        return Err("разрядность для двух держателей различается".into());
    }
    let (balance, _) = tokio::try_join!(
        original(ctx, t, wallet, field, h),
        original_for(ctx, t, wallet, control, verified[1], h)
    )?;
    // Меняем старший доступный бит, если все биты уже равны единице.
    let marker = if balance == field.max {
        field.max >> 1
    } else {
        field.max
    };
    let credit = field.credit(marker).ok_or("метка вне диапазона")?;
    let base = simulate::Base::for_chain(ctx.chain_id(), h.number.to(), h.timestamp.to());
    let res = simulate::run_anchored(ctx, &[call], &[], &[credit], base, Some(h.hash))
        .await
        .map_err(|e| e.message)?;
    if res.first().filter(|r| r.ok()).and_then(probe_calls::u256) != Some(marker) {
        return Err("одиночная метка полного слова не совпала с balanceOf".into());
    }
    Ok(Proof {
        identity,
        field,
        balance,
    })
}
#[cfg(test)]
impl Identity {
    pub(super) fn test_direct(runtime: B256) -> Self {
        Self {
            runtime,
            target: None,
            implementation_slot: false,
            admin: None,
        }
    }
}
#[cfg(test)]
#[path = "presell_layout_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "presell_layout_rpc_tests.rs"]
mod rpc_tests;
