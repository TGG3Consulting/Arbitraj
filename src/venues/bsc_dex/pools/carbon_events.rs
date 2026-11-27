//! Carbon strategies can be indexed or carried in event data. Global fee and controller upgrades affect every active pair.
use super::{
    carbon::{self, ICarbon as C},
    discover::Found,
    family_variant::Variant,
    listen::Pools,
};
use crate::core::types::{PoolRef, PoolState};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256};

pub(super) fn topics() -> Vec<B256> {
    vec![
        C::StrategyCreated::SIGNATURE_HASH,
        C::StrategyUpdated::SIGNATURE_HASH,
        C::StrategyDeleted::SIGNATURE_HASH,
        C::TradingFeePPMUpdated::SIGNATURE_HASH,
        C::PairTradingFeePPMUpdated::SIGNATURE_HASH,
        C::Upgraded::SIGNATURE_HASH,
    ]
}
pub(super) fn singleton(log: &Log) -> Option<PoolRef> {
    let topic = *log.topic0()?;
    let word = if topic == C::StrategyUpdated::SIGNATURE_HASH {
        log.topics().get(1)?.as_slice()
    } else if topic == C::StrategyCreated::SIGNATURE_HASH
        || topic == C::StrategyDeleted::SIGNATURE_HASH
    {
        log.inner.data.data.get(..32)?
    } else {
        return None;
    };
    let id = u128::from_be_bytes(word.get(..16)?.try_into().ok()?);
    (id != 0).then(|| PoolRef::Singleton {
        manager: log.address(),
        id: carbon::pair_id(id),
    })
}
/// These three events have no strategy ID. An older price may no longer be executable. The head refresh restores it.
pub(super) fn invalidated(p: &Pools, items: &[(Found, String)], log: &Log) -> bool {
    let Some(topic) = log.topic0() else {
        return false;
    };
    let pair = if *topic == C::PairTradingFeePPMUpdated::SIGNATURE_HASH {
        let [_, a, b] = log.topics() else {
            return false;
        };
        Some([Address::from_word(*a), Address::from_word(*b)])
    } else if *topic == C::TradingFeePPMUpdated::SIGNATURE_HASH
        || *topic == C::Upgraded::SIGNATURE_HASH
    {
        None
    } else {
        return false;
    };
    let mut handled = false;
    for (f, _) in items {
        if f.family.variant != Variant::Carbon || f.reference.address() != log.address() {
            continue;
        }
        handled = true;
        p.reg.update(&f.reference, |e| {
            if let PoolState::Carbon(s) = &e.pool.state
                && pair.is_none_or(|a| {
                    let mut b = carbon::raw(s);
                    b.sort_unstable();
                    b == a || b == [a[1], a[0]]
                })
                && (log.removed
                    || log
                        .block_number
                        .zip(log.log_index)
                        .is_none_or(|(n, i)| !e.has_applied(n, i)))
            {
                e.valid = false;
            }
        });
    }
    handled
}
