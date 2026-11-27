//! Фоновая проверка ERC-20. Наблюдение двух balanceOf выбирает конкретный
//! адрес хранения и ключ. Код getter должен вернуть всё слово без изменений.
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
mod observed;
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
}
impl BalanceField {
    /// Используется только в той же симуляции и на том же кошельке, что и проверка.
    pub(super) fn credit(self, amount: U256) -> simulate::Override {
        simulate::Override {
            address: self.storage,
            code: Bytes::new(),
            state: vec![(self.key, B256::from(amount.to_be_bytes::<32>()))],
            diff: true,
        }
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
async fn word(ctx: &Ctx, address: Address, key: B256, h: Head) -> Result<B256, String> {
    rpc::call(ctx, "eth_getStorageAt", json!([address, key, h.selector()]))
        .await
        .map_err(|e| e.message)
}
/// Привязка внешнего кода и служебных чтений подтверждается заново на каждом H.
async fn binding(
    ctx: &Ctx,
    t: Address,
    h: Head,
    reads: Reads,
    generation: u64,
    cache: &Cache,
) -> Result<(Identity, Code, Option<Code>), String> {
    let token = cache.code(ctx, t, generation, h).await?;
    let other = async {
        match reads.other {
            Some(a) => cache.code(ctx, a, generation, h).await.map(Some),
            None => Ok(None),
        }
    };
    let implementation = async {
        if reads.implementation {
            let actual = word(ctx, t, IMPL_SLOT, h).await?;
            if reads.other.is_none_or(|a| actual != a.into_word()) {
                return Err("изменилась реализация прокси".into());
            }
        }
        Ok(())
    };
    let admin = async {
        if reads.admin {
            word(ctx, t, ADMIN_SLOT, h).await.map(Some)
        } else {
            Ok(None)
        }
    };
    let (other, (), admin) = tokio::try_join!(other, implementation, admin)?;
    let target = other.as_ref().map(|c| (c.account, keccak256(&c.bytecode)));
    Ok((
        Identity {
            runtime: keccak256(&token.bytecode),
            target,
            implementation_slot: reads.implementation,
            admin,
        },
        token,
        other,
    ))
}
async fn original(
    ctx: &Ctx,
    t: Address,
    wallet: Address,
    field: BalanceField,
    h: Head,
) -> Result<U256, String> {
    let tx = json!({"from":wallet,"to":t,"data":probe_calls::balance(wallet,t,wallet).input});
    let read = async {
        rpc::call::<Bytes>(ctx, "eth_call", json!([tx, h.selector()]))
            .await
            .map_err(|e| e.message)
    };
    let (b, w) = tokio::try_join!(read, word(ctx, field.storage, field.key, h))?;
    balance_word(&b, w)
}
pub(super) async fn verify(
    ctx: &Ctx,
    t: Address,
    wallet: Address,
    h: Head,
    generation: u64,
    cache: &Cache,
    previous: Option<(Identity, BalanceField)>,
) -> Result<Proof, String> {
    if let Some((previous, field)) = previous {
        let reads = Reads {
            other: previous.target.map(|(a, _)| a),
            implementation: previous.implementation_slot,
            admin: previous.admin.is_some(),
        };
        let (identity, _, _) = binding(ctx, t, h, reads, generation, cache).await?;
        if identity == previous {
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
    let (fields, reads) = observed::compare(&a, &b, t)?;
    let (identity, token, implementation) = binding(ctx, t, h, reads, generation, cache).await?;
    for (holder, field) in [wallet, control].into_iter().zip(fields) {
        if !(Read {
            token: &token,
            implementation: implementation.as_ref(),
            admin: identity.admin,
            caller: wallet,
            holder,
            key: field.key,
            storage: field.storage,
        })
        .proves()
        {
            return Err("не подтверждён прямой возврат целого слова из balanceOf".into());
        }
    }
    let field = fields[0];
    let balance = original(ctx, t, wallet, field, h).await?;
    let marker = if balance == U256::MAX {
        U256::MAX ^ (U256::from(1) << 255)
    } else {
        U256::MAX
    };
    let base = simulate::Base::for_chain(ctx.chain_id(), h.number.to(), h.timestamp.to());
    let res = simulate::run_anchored(
        ctx,
        &[call],
        &[],
        &[field.credit(marker)],
        base,
        Some(h.hash),
    )
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
