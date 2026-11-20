//! Файл: venues/bsc_dex/pools/aero_fee_seen.rs
//! Что делает: решение 470 (ревью t240) — комиссия пула Aero Slipstream V3 из чтения СТАРОГО блока не перетирает
//! комиссию более нового. Комиссию пишут два пути: лёгкое чтение каждым блоком (per_block_aero.rs, голова) и полное
//! чтение по событию (load.rs → aero_cl.rs fee, блок события — бывает старше головы). Память: (площадка, пул) →
//! (блок, комиссия) самого нового чтения; чтение блока старше памяти получает комиссию из памяти.
//! Кто вызывает: pools/aero_cl.rs (fee), pools/per_block_aero.rs.
//! От чего зависит: std.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use alloy_primitives::Address;

/// (площадка, пул) → (блок, комиссия) самого нового чтения.
type Seen = HashMap<(&'static str, Address), (u64, u32)>;

static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Комиссия для записи: чтение блока `block` не старше памяти — оно и запоминается; старше — комиссия из памяти
/// (более новый блок). Принимает: id площадки, пул, блок чтения, прочитанную комиссию. Ошибок нет.
pub fn newest(place: &'static str, pool: Address, block: u64, fee: u32) -> u32 {
    let mut m = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    match m.get(&(place, pool)) {
        Some(&(b, f)) if b > block => f,
        _ => {
            m.insert((place, pool), (block, fee));
            fee
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Новое чтение запоминается; чтение старого блока получает комиссию нового; тот же блок — перезаписывает.
    #[test]
    fn old_block_does_not_override() {
        let p = Address::repeat_byte(0x47);
        assert_eq!(newest("test_seen", p, 10, 500), 500);
        assert_eq!(newest("test_seen", p, 9, 300), 500);
        assert_eq!(newest("test_seen", p, 10, 400), 400);
        assert_eq!(newest("test_seen", p, 11, 450), 450);
    }
}
