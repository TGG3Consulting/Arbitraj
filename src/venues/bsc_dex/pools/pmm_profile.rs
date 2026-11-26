//! Рабочие роли, токены и ожидаемый runtime PMM из настроек выбранного DEX.
use super::{families::Proto, pmm_attest};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::{Address, B256};
use serde_json::Value;
use std::{collections::BTreeSet, str::FromStr};

pub(super) struct Profile {
    generation: u64,
    pub pool: Address,
    pub implementation: Address,
    pub oracle: Address,
    pub vault: Address,
    pub counter: Address,
    pub bases: Vec<Address>,
    pub(super) code: Vec<(Address, B256)>,
    pub(super) immutables: Vec<(usize, Address)>,
    pub(super) proxies: Vec<(Address, Address)>,
}

fn unique<T: Ord + Copy + Default>(values: &[T]) -> bool {
    let mut seen = BTreeSet::new();
    !values.is_empty() && values.iter().all(|v| *v != T::default() && seen.insert(*v))
}
fn list<T: FromStr + Ord + Copy + Default>(values: Vec<String>) -> Option<Vec<T>> {
    let v: Vec<T> = values
        .iter()
        .map(|s| s.trim().parse::<T>().ok())
        .collect::<Option<_>>()?;
    unique(&v).then_some(v)
}

impl Profile {
    pub fn pair(&self, coin: Address, counter: Address) -> bool {
        counter == self.counter && self.bases.contains(&coin)
    }

    pub fn unordered(&self, a: Address, b: Address) -> bool {
        self.pair(a, b) || self.pair(b, a)
    }

    pub fn read(ctx: &Ctx, id: &str, proto: Proto) -> GatewayResult<Self> {
        let generation = ctx.settings.generation();
        let wrong = || {
            ctx.err(
                ErrorCode::NotApplicable,
                format!("{id}: неполные или противоречивые роли PMM"),
            )
        };
        let addr = |key| ctx.dex_addr(key, id);
        let pool = addr("dex.pmm_pool")?;
        let implementation = addr("dex.pool_implementation")?;
        let counter = addr("dex.pmm_counter")?;
        let bases = list(ctx.dex_list("dex.pmm_base_tokens", id)?).ok_or_else(wrong)?;
        let hashes: Vec<B256> =
            list(ctx.dex_list("dex.pmm_runtime_hashes", id)?).ok_or_else(wrong)?;
        let mut roles = vec![pool, implementation, counter];
        roles.extend(&bases);
        let (oracle, vault) = if proto == Proto::Lunar {
            if bases.as_slice() != [ctx.addr("dex.native_wrapped")?] {
                return Err(wrong());
            }
            (Address::ZERO, Address::ZERO)
        } else {
            let (oracle, vault) = (addr("dex.oracle")?, addr("dex.vault")?);
            roles.extend([oracle, vault]);
            (oracle, vault)
        };
        let mut immutables = Vec::new();
        if proto == Proto::Elfomo {
            for (key, target) in [
                ("dex.pmm_pricing_offsets", oracle),
                ("dex.pmm_vault_offsets", vault),
            ] {
                let offsets: Vec<usize> = list(ctx.dex_list(key, id)?).ok_or_else(wrong)?;
                immutables.extend(offsets.into_iter().map(|n| (n, target)));
            }
            immutables.sort_unstable_by_key(|(n, _)| *n);
            if immutables.windows(2).any(|w| w[1].0 - w[0].0 <= 32) {
                return Err(wrong());
            }
        }
        let (code, proxies) = match (proto, hashes.as_slice()) {
            (Proto::Lunar, [proxy, imp]) if bases.len() == 1 => (
                vec![(pool, *proxy), (implementation, *imp)],
                vec![(pool, implementation)],
            ),
            (Proto::Elfomo, [router, pricing, imp]) => (
                vec![(pool, *router), (oracle, *pricing), (implementation, *imp)],
                vec![(oracle, implementation)],
            ),
            (Proto::BaiBai, [proxy, entry, book, custodian]) => {
                let aux: Vec<Address> =
                    list(ctx.dex_list("dex.pmm_aux_implementations", id)?).ok_or_else(wrong)?;
                roles.extend(&aux);
                let [book_impl, vault_impl] = aux.as_slice() else {
                    return Err(wrong());
                };
                (
                    vec![
                        (pool, *proxy),
                        (oracle, *proxy),
                        (vault, *proxy),
                        (implementation, *entry),
                        (*book_impl, *book),
                        (*vault_impl, *custodian),
                    ],
                    vec![
                        (pool, implementation),
                        (oracle, *book_impl),
                        (vault, *vault_impl),
                    ],
                )
            }
            _ => return Err(wrong()),
        };
        if !unique(&roles) || generation != ctx.settings.generation() {
            return Err(wrong());
        }
        Ok(Self {
            generation,
            immutables,
            pool,
            implementation,
            oracle,
            vault,
            counter,
            bases,
            code,
            proxies,
        })
    }

    pub fn current(&self, ctx: &Ctx) -> GatewayResult<()> {
        if ctx.settings.generation() != self.generation {
            return Err(ctx.err(
                ErrorCode::StaleData,
                "Настройки изменились во время чтения PMM",
            ));
        }
        Ok(())
    }

    pub async fn verify_code(&self, ctx: &Ctx, tag: &Value) -> GatewayResult<()> {
        pmm_attest::verify(self, ctx, tag).await?;
        self.current(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_accept_replacements_and_reject_duplicate_zero_or_malformed_roles() {
        let addr = |i| Address::with_last_byte(i);
        for replacement in [1, 8] {
            assert_eq!(
                list::<Address>(vec![addr(replacement).to_string(), addr(2).to_string()]),
                Some(vec![addr(replacement), addr(2)])
            );
        }
        assert!(list::<Address>(vec![addr(1).to_string(), addr(1).to_string()]).is_none());
        assert!(list::<Address>(vec![Address::ZERO.to_string()]).is_none());
        assert!(list::<B256>(vec![addr(1).to_string()]).is_none());
    }
}
