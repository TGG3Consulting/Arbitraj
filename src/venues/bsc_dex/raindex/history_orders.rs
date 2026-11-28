//! Проверенный снимок открытых ордеров и изменения одной книги V5 в порядке EVM.
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

pub(super) fn topics() -> [B256; 2] {
    [
        wire::AddOrderV3::SIGNATURE_HASH,
        wire::RemoveOrderV3::SIGNATURE_HASH,
    ]
}

/// Валидация всего диапазона предшествует изменению позиции и списка активных ордеров.
pub(super) fn changes(book: Address, start: u64, end: u64, logs: &mut [Log]) -> Option<Delta> {
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
            || l.address() != book
            || l.topics().len() != 1
        {
            return None;
        }
        previous = Some(pos);
        let (sender, hash, order, add) = match l.topic0()? {
            t if *t == wire::AddOrderV3::SIGNATURE_HASH => {
                let e = wire::AddOrderV3::decode_log_data(&l.inner.data).ok()?;
                (e.sender, e.orderHash, e.order, true)
            }
            t if *t == wire::RemoveOrderV3::SIGNATURE_HASH => {
                let e = wire::RemoveOrderV3::decode_log_data(&l.inner.data).ok()?;
                (e.sender, e.orderHash, e.order, false)
            }
            _ => return None,
        };
        if sender != order.owner {
            return None;
        }
        let bytes: Bytes = order.abi_encode().into();
        wire::Order::decode(5, &bytes, hash)?;
        delta.insert(hash, add.then_some(bytes));
    }
    Some(delta)
}
