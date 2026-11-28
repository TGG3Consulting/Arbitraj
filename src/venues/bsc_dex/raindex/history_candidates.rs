//! Стартовые кандидаты из неполной выборки. Их байты не задают позицию исторического сканера.
use super::{history_orders::Cursor, source::Raw, wire::Order, *};
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Clone, Default)]
pub(super) struct Indexed {
    pub cursor: Option<Cursor>,
    pub rows: Arc<Raw>,
}

#[derive(Deserialize)]
struct Candidate {
    hash: B256,
    order: Bytes,
}
#[derive(Deserialize)]
struct Book {
    version: u8,
    book: Address,
    book_hash: B256,
    deployment: u64,
    orders: Vec<Candidate>,
}
#[derive(Deserialize)]
struct Evidence {
    chain_id: u64,
    block: u64,
    books: Vec<Book>,
}
type Key = (u8, Address, B256, u64);
struct Candidates {
    chain: u64,
    block: u64,
    books: HashMap<Key, Raw>,
}

fn candidates() -> Option<&'static Candidates> {
    static DATA: OnceLock<Option<Candidates>> = OnceLock::new();
    DATA.get_or_init(|| {
        let input = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/raindex_base_candidate_programs.json.gz");
        let saved: Evidence = serde_json::from_reader(flate2::read::GzDecoder::new(input.as_slice())).ok()?;
        let mut books = HashMap::new();
        for book in saved.books {
            let mut rows = Raw::new();
            for r in book.orders {
                Order::decode(book.version, &r.order, r.hash)?;
                if rows.insert(r.hash, r.order).is_some() {
                    return None;
                }
            }
            let key = (book.version, book.book, book.book_hash, book.deployment);
            if books.insert(key, rows).is_some() {
                return None;
            }
        }
        Some(Candidates { chain: saved.chain_id, block: saved.block, books })
    }).as_ref()
}

pub(super) struct View {
    indexed: Arc<Raw>,
    partial: Option<&'static Raw>,
}

impl View {
    pub(super) fn complete(indexed: Arc<Raw>) -> Self {
        Self { indexed, partial: None }
    }

    pub(super) fn for_book(p: Profile, from: u64, height: u64, indexed: &Indexed) -> Self {
        let partial = (p.chain == 8453).then(candidates).flatten()
            .filter(|s| s.chain == p.chain && height >= s.block
                && indexed.cursor.is_none_or(|(block, _)| block < s.block || block > height))
            .and_then(|s| s.books.get(&(p.version, p.book, p.book_hash, from)));
        Self { indexed: indexed.rows.clone(), partial }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&B256, &Bytes)> {
        self.indexed.iter().chain(
            self.partial.into_iter().flatten().filter(|(h, _)| !self.indexed.contains_key(*h))
        )
    }
}

