//! Проверенный снимок открытых ордеров и изменения одной книги в порядке EVM.
use super::{source::Raw, wire, *};
use alloy::{
    rpc::types::Log,
    sol_types::{SolEvent, SolValue},
};
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

pub(super) type Delta = HashMap<B256, Option<Bytes>>;
pub(super) type Cursor = (u64, B256);
#[derive(Deserialize)]
struct Record {
    hash: B256,
    order: Bytes,
}
#[derive(Deserialize)]
struct Seed {
    chain_id: u64,
    version: u8,
    book: Address,
    book_hash: B256,
    block: u64,
    block_hash: B256,
    orders: Vec<Record>,
}
struct Snapshot {
    key: (u64, u8, Address, B256),
    cursor: Cursor,
    orders: Arc<Raw>,
}

pub(super) fn seed(p: Profile, deployment: u64) -> Option<(Cursor, Arc<Raw>)> {
    if p.version != 5 || p.chain != 8453 {
        return None;
    }
    static SEED: OnceLock<Option<Snapshot>> = OnceLock::new();
    let s = SEED
        .get_or_init(|| {
            let data = include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_v5_seed.json.gz"
            );
            let s: Seed =
                serde_json::from_reader(flate2::read::GzDecoder::new(data.as_slice())).ok()?;
            let mut orders = Raw::new();
            for record in s.orders {
                wire::Order::decode(s.version, &record.order, record.hash)?;
                if orders.insert(record.hash, record.order).is_some() {
                    return None;
                }
            }
            Some(Snapshot {
                key: (s.chain_id, s.version, s.book, s.book_hash),
                cursor: (s.block, s.block_hash),
                orders: Arc::new(orders),
            })
        })
        .as_ref()?;
    (s.key == (p.chain, p.version, p.book, p.book_hash) && deployment <= s.cursor.0)
        .then(|| (s.cursor, s.orders.clone()))
}

pub(super) fn topics(version: u8) -> Option<[B256; 2]> {
    Some(match version {
        3 => [wire::AddOrder::SIGNATURE_HASH, wire::RemoveOrder::SIGNATURE_HASH],
        4 => [wire::AddOrderV2::SIGNATURE_HASH, wire::RemoveOrderV2::SIGNATURE_HASH],
        5..=7 => [wire::AddOrderV3::SIGNATURE_HASH, wire::RemoveOrderV3::SIGNATURE_HASH],
        _ => return None,
    })
}

fn order_event(version: u8, added: bool, log: &Log) -> Option<(B256, Bytes)> {
    // В каждом поколении оба события включают полную программу и заявленный хеш.
    macro_rules! decoded {
        ($event:ty) => {{
            let e = <$event>::decode_log_data_validate(&log.inner.data).ok()?;
            if e.sender != e.order.owner {
                return None;
            }
            let raw: Bytes = e.order.abi_encode().into();
            if raw.len() > 65_536 || alloy_primitives::keccak256(&raw) != e.orderHash {
                return None;
            }
            Some((e.orderHash, raw))
        }};
    }
    match (version, added) {
        (3, true) => decoded!(wire::AddOrder),
        (3, false) => decoded!(wire::RemoveOrder),
        (4, true) => decoded!(wire::AddOrderV2),
        (4, false) => decoded!(wire::RemoveOrderV2),
        (5..=7, true) => decoded!(wire::AddOrderV3),
        (5..=7, false) => decoded!(wire::RemoveOrderV3),
        _ => None,
    }
}

/// Валидация всего диапазона предшествует изменению позиции и списка активных ордеров.
pub(super) fn changes(profile: Profile, start: u64, end: u64, logs: &mut [Log]) -> Option<Delta> {
    let [add, remove] = topics(profile.version)?;
    logs.sort_by_key(|l| (l.block_number, l.log_index));
    let mut previous = None;
    let mut delta = Delta::new();
    for l in logs {
        let pos = (l.block_number?, l.log_index?);
        if !(start..=end).contains(&pos.0)
            || previous == Some(pos)
            || l.removed
            || l.block_hash.is_none()
            || l.transaction_hash.is_none()
            || l.address() != profile.book
            || l.topics().len() != 1
        {
            return None;
        }
        previous = Some(pos);
        let topic = *l.topic0()?;
        if topic != add && topic != remove {
            return None;
        }
        let added = topic == add;
        let (hash, bytes) = order_event(profile.version, added, l)?;
        delta.insert(hash, added.then_some(bytes));
    }
    Some(delta)
}
