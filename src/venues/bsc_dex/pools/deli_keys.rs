//! Прообразы PoolId Deli из настроек и из Initialize. Событие сохраняет только ключ;
//! проверка реестра, ролей и выплат выполняется при последующем чтении.
use super::{
    abi_check::IPositionManagerKeys,
    deli::invalid,
    families::Family,
    key::{PoolKey, UniswapPoolKeyData},
};
use crate::gateway::errors::GatewayResult;
use crate::venues::bsc_dex::ctx::Ctx;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, keccak256};

pub(super) fn key(bytes: &[u8], concentrated: bool) -> Option<PoolKey> {
    if bytes.len() != 160 {
        return None;
    }
    let k = IPositionManagerKeys::poolKeysCall::abi_decode_returns_validate(bytes).ok()?;
    let fee: u32 = k.fee.to();
    if k.currency0.is_zero()
        || k.currency0 >= k.currency1
        || k.hooks.is_zero()
        || (concentrated && fee != 0x800000)
        || (!concentrated && fee >= 1_000_000)
        || k.tickSpacing.as_i32() <= 0
    {
        return None;
    }
    Some(PoolKey::Uniswap(UniswapPoolKeyData {
        currency0: k.currency0,
        currency1: k.currency1,
        hooks: k.hooks,
        fee,
        tick_spacing: k.tickSpacing,
    }))
}

pub(super) fn configured(ctx: &Ctx, f: &Family, id: B256) -> GatewayResult<PoolKey> {
    let keys = match ctx.dex_list("dex.deli_pool_keys", f.id) {
        Ok(keys) => keys,
        Err(e) if e.code == crate::gateway::errors::ErrorCode::NotConfigured => Vec::new(),
        Err(e) => return Err(e),
    };
    let mut selected = None;
    for encoded in keys {
        let raw: Bytes = encoded.parse().map_err(|_| invalid(ctx))?;
        if keccak256(&raw) == id {
            if selected.is_some() {
                return Err(invalid(ctx));
            }
            selected = key(&raw, false);
            if selected.is_none() {
                return Err(invalid(ctx));
            }
        }
    }
    // Здесь только прообраз хеша: право использовать рынок отдельно проверяет market на текущем блоке.
    selected
        .or_else(|| preimage(id))
        .ok_or_else(|| invalid(ctx))
}

pub(super) fn bindings(ctx: &Ctx, f: &Family) -> GatewayResult<[(Address, B256); 4]> {
    let roles = ctx.dex_list("dex.deli_role_bindings", f.id)?;
    let mut result = [(Address::ZERO, B256::ZERO); 4];
    if roles.len() != result.len() {
        return Err(invalid(ctx));
    }
    for (i, item) in roles.iter().enumerate() {
        let (a, h) = item.split_once('@').ok_or_else(|| invalid(ctx))?;
        let a: Address = a.parse().map_err(|_| invalid(ctx))?;
        let h: B256 = h.parse().map_err(|_| invalid(ctx))?;
        if a.is_zero() || h.is_zero() || h == keccak256([]) || result[..i].iter().any(|r| r.0 == a)
        {
            return Err(invalid(ctx));
        }
        result[i] = (a, h);
    }
    Ok(result)
}

struct Claimed {
    store: crate::settings::store::Store,
    generation: u64,
    venue: &'static str,
    hooks: Vec<Address>,
}
thread_local! {
    static CLAIMED: std::cell::RefCell<Option<Claimed>>=const { std::cell::RefCell::new(None) };
}
pub(super) fn claimed(ctx: &Ctx, hook: Address) -> bool {
    if hook.is_zero() {
        return false;
    }
    CLAIMED.with_borrow_mut(|slot| {
        let generation = ctx.settings.generation();
        if let Some(row) = slot
            && row.generation == generation
            && row.store.same(&ctx.settings)
            && row.venue == ctx.place.id
        {
            return row.hooks.contains(&hook);
        }
        let hooks: Vec<_> = super::families::all(ctx)
            .into_iter()
            .filter(|f| f.variant.deli())
            .filter_map(|f| bindings(ctx, f).ok().map(|roles| roles[0].0))
            .collect();
        let contains = hooks.contains(&hook);
        *slot = Some(Claimed {
            store: ctx.settings.clone(),
            generation,
            venue: ctx.place.id,
            hooks,
        });
        contains
    })
}

/// Сохраняет неизменяемый прообраз PoolId. Повтор в другой ветви с тем же хешем не может
/// изменить ключ. Доказательство инициализации и прав хука всегда читается заново.
static OBSERVED: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<B256, PoolKey>>> =
    std::sync::LazyLock::new(Default::default);

pub(super) fn preimage(id: B256) -> Option<PoolKey> {
    OBSERVED.read().ok()?.get(&id).cloned()
}

pub(super) fn observed(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    pk: &PoolKey,
) -> GatewayResult<bool> {
    let expected = ctx.dex_addr("dex.pool_manager", f.id)?;
    let Some(valid) = observed_key(
        (expected, bindings(ctx, f)?[0].0),
        manager,
        pk,
        f.variant == super::family_variant::Variant::DeliCl,
    ) else {
        return Ok(false);
    };
    OBSERVED
        .write()
        .map_err(|_| invalid(ctx))?
        .entry(valid.id())
        .or_insert(valid);
    Ok(true)
}

fn observed_key(
    expected: (Address, Address),
    manager: Address,
    pk: &PoolKey,
    concentrated: bool,
) -> Option<PoolKey> {
    (expected == (manager, pk.hooks()))
        .then(|| key(&pk.abi(), concentrated))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::aliases::I24;
    #[test]
    fn new_initialized_key_requires_the_bound_hook_and_manager() {
        let manager = Address::repeat_byte(4);
        let hook = Address::repeat_byte(5);
        let pk = PoolKey::Uniswap(UniswapPoolKeyData {
            currency0: Address::repeat_byte(1),
            currency1: Address::repeat_byte(2),
            fee: 2450,
            tick_spacing: I24::try_from(7).unwrap(),
            hooks: hook,
        });
        assert_eq!(
            observed_key((manager, hook), manager, &pk, false),
            Some(pk.clone())
        );
        assert!(observed_key((manager, hook), Address::repeat_byte(9), &pk, false).is_none());
        assert!(observed_key((manager, Address::repeat_byte(7)), manager, &pk, false).is_none());
        assert_eq!(key(&pk.abi(), false).unwrap().id(), pk.id());
        assert!(observed_key((manager, hook), manager, &pk, true).is_none());
        let PoolKey::Uniswap(mut data) = pk else {
            unreachable!()
        };
        data.fee = 0x800000;
        let cl = PoolKey::Uniswap(data);
        assert!(observed_key((manager, hook), manager, &cl, false).is_none());
        assert_eq!(observed_key((manager, hook), manager, &cl, true), Some(cl));
    }
}
