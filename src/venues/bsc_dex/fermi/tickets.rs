//! Одноразовые билеты слота. Обычная котировка и котировка денежного шага
//! подтверждают условия по отдельности. Первое исполнение погашает общий билет кошелька.
use super::Prepared;
use alloy::primitives::{Address, B256, keccak256};
use alloy::sol_types::SolValue;
use std::{collections::HashMap, sync::{Arc, Mutex}};

type Key = (Address, B256);

struct Issued<T> {
    ordinary: Option<T>,
    settlement: Option<T>,
}

impl<T> Issued<T> {
    fn for_step(&mut self, settlement: bool) -> &mut Option<T> {
        if settlement { &mut self.settlement } else { &mut self.ordinary }
    }
}

struct WalletTickets<T>(HashMap<Key, Issued<T>>);

impl<T> Default for WalletTickets<T> {
    fn default() -> Self { Self(HashMap::new()) }
}

impl<T> WalletTickets<T> {
    fn issue(&mut self, key: Key, settlement: bool, quote: T) {
        let issued = self.0.entry(key).or_insert(Issued { ordinary: None, settlement: None });
        *issued.for_step(settlement) = Some(quote);
    }

    /// Запрос чужого назначения сохраняет обе котировки. Успех погашает обе под тем же ключом.
    fn take<E>(&mut self, key: Key, settlement: bool, validate: impl FnOnce(&T) -> Result<(), E>) -> Result<Option<T>, E> {
        let Some(issued) = self.0.get_mut(&key) else { return Ok(None) };
        let slot = issued.for_step(settlement);
        let Some(quote) = slot.as_ref() else { return Ok(None) };
        validate(quote)?;
        let quote = slot.take();
        self.0.remove(&key);
        Ok(quote)
    }
}

#[derive(Default, Clone)]
pub(in crate::venues::bsc_dex::fermi) struct Tickets(Arc<Mutex<WalletTickets<Arc<Prepared>>>>);

impl Tickets {
    pub(super) fn save(&self, q: Prepared, settlement: bool) -> B256 {
        let id = keccak256((
            q.proof.calldata(),
            q.proof.payer,
            q.proof.executor,
            q.proof.builder.clone(),
            q.fees.max_fee,
            q.fees.tip,
        ).abi_encode());
        let mut tickets = self.0.lock().unwrap_or_else(|e| e.into_inner());
        tickets.0.retain(|_, v| v.ordinary.iter().chain(&v.settlement).any(|q| q.book.current()));
        tickets.issue((q.proof.payer, id), settlement, Arc::new(q));
        id
    }

    pub(in crate::venues::bsc_dex::fermi) fn take<E>(
        &self, who: Address, id: B256, settlement: bool, validate: impl FnOnce(&Prepared) -> Result<(), E>,
    ) -> Result<Option<Arc<Prepared>>, E> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take((who, id), settlement, |q| validate(q))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_quotes_share_one_claim_and_stale_or_wrong_scope_preserves_it() {
        let mut book = WalletTickets::default();
        let key = (Address::repeat_byte(1), B256::repeat_byte(2));
        let current = |revision: &u64| if *revision == 2 { Ok(()) } else { Err("устарело") };
        book.issue(key, true, 2);
        assert_eq!(book.take(key, false, current), Ok(None));
        book.issue(key, false, 1);
        assert_eq!(book.take(key, false, current), Err("устарело"));
        assert_eq!(book.take(key, true, current), Ok(Some(2)));
        assert_eq!(book.take(key, false, |_| Ok::<_, ()>(())), Ok(None));

        // Две подготовки одного calldata допускают свои режимы и разделяют одну выдачу.
        let shared = Mutex::new(book);
        std::thread::scope(|scope| {
            let direct = scope.spawn(|| shared.lock().unwrap().issue(key, false, 2));
            let mixed = scope.spawn(|| shared.lock().unwrap().issue(key, true, 2));
            direct.join().unwrap();
            mixed.join().unwrap();
            let a = scope.spawn(|| shared.lock().unwrap().take(key, false, current).unwrap());
            let b = scope.spawn(|| shared.lock().unwrap().take(key, true, current).unwrap());
            assert_eq!([a.join().unwrap(), b.join().unwrap()].into_iter().flatten().count(), 1);
        });
    }
}
