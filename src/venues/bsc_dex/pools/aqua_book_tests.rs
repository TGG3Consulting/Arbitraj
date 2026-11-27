use super::{Book, IAqua, Program, Roles};
use alloy::{
    rpc::types::Log,
    sol_types::{SolEvent, SolValue},
};
use alloy_primitives::{Address, Bytes, U256, keccak256};

#[test]
fn duplicates_notify_each_watcher_and_only_changed_rows_are_persisted() {
    let maker = Address::repeat_byte(1);
    let roles = Roles {
        registry: Address::repeat_byte(2),
        engine: Address::repeat_byte(3),
    };
    let a = Address::repeat_byte(4);
    let b = Address::repeat_byte(5);
    let c = Address::repeat_byte(6);
    let p = Program {
        maker,
        traits: U256::ONE << 254,
        data: Bytes::from_static(&[7]),
    };
    let raw = Bytes::from(p.abi_encode());
    let h = keccak256(&raw);
    let mut book = Book::default();
    assert!(book.insert(maker, h, raw));
    assert!(book.tokens(maker, h, &[a]));
    let mut log = Log::default();
    log.inner.address = roles.registry;
    log.inner.data = IAqua::Pushed {
        maker,
        app: roles.engine,
        strategyHash: h,
        token: b,
        amount: U256::ONE,
    }
    .encode_log_data();
    assert_eq!(book.observe(roles, &log), Some((h, true)));
    assert_eq!(book.observe(roles, &log), Some((h, false)));
    assert_eq!(book.pairs((a, b)).count(), 2);
    assert_eq!(book.pairs((a, c)).count(), 0);
    let delta = book.selection(&[h].into());
    let rows = delta.persist().unwrap();
    assert_eq!(rows.len(), 1);
    let mut restored = Book::default();
    assert!(restored.restore(h, &rows[0].1).is_some());
    assert_eq!(
        restored.pairs((b, a)).collect::<Vec<_>>(),
        vec![(h, false), (h, true)]
    );
    log.removed = true;
    assert_eq!(restored.observe(roles, &log), None);
}
