//! Файл: venues/bsc_dex/pools/registry_key.rs
//! Что делает: решение 281 — «ключ реестра фабрики» V3-форка, у которого фабрика хранит пул НЕ под fee() пула. SquadSwap
//! V3: фабрика кладёт пул в `getPool[token0][token1][fee]` под комиссией создания (SquadV3Factory.sol L74-L78), пул
//! хранит её в неизменяемом `defaultFee` (SquadV3Pool.sol L49, L130), а `fee()` пула — это
//! `poolManager.getFee(pool, defaultFee)` (L136-L137): своя комиссия менеджера, если включена, иначе defaultFee. Значит,
//! getPool(t0, t1, fee()) при своей комиссии даёт 0 (запись 15.11.2026: fee() = 200, defaultFee() = 100,
//! getPool(…, 200) = 0, getPool(…, 100) = пул), и официальный QuoterV2 (ищет пул через фабрику) с fee() откатывается.
//! При заданной dex.v3_registry_fee_method (сигнатура метода пула, например `defaultFee()`) — значение метода читается
//! при проверке пула вместе с полями (verify_calls.rs) и идёт в getPool реестра (вместо fee()) и в QuoterV2 (вместо
//! fee_pips пула); fee_pips записи — по-прежнему fee() (им считает математика). Ключ неизменяем у пула (`immutable`),
//! поэтому запоминается по адресу пула до перезапуска; нет ключа в памяти — квота не строится (не гадаем ключ).
//! Не задана — поведение прежнее: те же вызовы, getPool и квотер по fee().
//! Кто вызывает: pools/verify_calls.rs (field_calls), pools/verify.rs (from_answers, remember), quote/quoter_calls.rs
//! (QuoterV2).
//! От чего зависит: ctx.rs, families.rs, gateway/shard_map.rs (решение 409: память ключей по частям).
//! Настройки: dex.v3_registry_fee_method [DEX] (необязательная).
//!
//! Источник (verified, Sourcify partial_match, chain 56): пул SquadV3Pool 0x9fb9815ca1417b06466aaa86521a28ba83d0bd33,
//! SquadV3Pool.sol (sha256 88dd3da0…9283); фабрика SquadV3Factory 0x10d8612d9d8269e322ab551c18a307cb4d6bc07b,
//! SquadV3Factory.sol (sha256 064bd44a…fc06). Адреса фабрики и QuoterV2 —
//! https://docs.squadswap.com/platform/developers/smart-contracts/bsc-contracts.md

use std::sync::LazyLock;

use alloy_primitives::{Address, Bytes, keccak256};

use crate::gateway::shard_map::ShardMap;

use super::super::ctx::Ctx;
use super::families::{Family, Proto};

/// Имя настройки.
pub const SETTING: &str = "dex.v3_registry_fee_method";

/// Ключ реестра по адресу пула (значение метода, прочитанное при проверке). Решение 409 (t194, ревью М3): карта по
/// частям (gateway/shard_map.rs) — get (каждый запрос QuoterV2 к пулу V3) берёт замок только своей части, remember —
/// вставка без копии карты.
/// Решение 435 (433-и): ключ — (площадка DEX, пул): пул с тем же адресом в другой сети — другой контракт.
static KEYS: LazyLock<ShardMap<(&'static str, Address), u32>> = LazyLock::new(ShardMap::default);

/// Сигнатура метода ключа: Some только у вида V3 (ABI Uniswap или PancakeSwap) с заданной непустой настройкой.
/// Ошибок нет: не задана — None (поведение прежнее).
pub fn method(ctx: &Ctx, f: &Family) -> Option<String> {
    if !matches!(f.proto, Proto::UniswapV3 | Proto::PancakeV3) {
        return None;
    }
    ctx.dex_text(SETTING, f.id)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Данные вызова метода без аргументов: селектор = первые 4 байта keccak256 сигнатуры (как dex.v2_fee_method). Ошибок нет.
pub fn selector(sig: &str) -> Bytes {
    Bytes::copy_from_slice(&keccak256(sig.as_bytes())[..4])
}

/// Ключ из ответа метода: одно слово ABI, значение — uint24 (ключ getPool фабрики V3). Ошибка — причина.
pub fn parse(b: &[u8]) -> Result<u32, String> {
    if b.len() < 32 {
        return Err("метод ключа реестра вернул не число".into());
    }
    if b[..29].iter().any(|x| *x != 0) {
        return Err("метод ключа реестра вернул не uint24".into());
    }
    Ok(u32::from_be_bytes([0, b[29], b[30], b[31]]))
}

/// Ключ из ответов полей пула (порядок — verify_calls::field_calls): седьмой ответ есть только у V3 с настройкой.
/// Принимает: ответы. Возвращает: None — вызова не было (поведение прежнее). Ошибка — метод откатился или не uint24:
/// пул не берётся с причиной.
pub fn from_answers(got: &[Result<Bytes, String>]) -> Result<Option<u32>, String> {
    match got.get(6) {
        None => Ok(None),
        Some(Err(w)) => Err(format!("{SETTING}: метод пула откатился: {w}")),
        Some(Ok(b)) => parse(b).map(Some),
    }
}

/// Запомнить ключ пула (проверка прошла: реестр фабрики по этому ключу вернул сам пул). Принимает: площадку DEX, пул,
/// ключ. Ошибок нет.
pub fn remember(venue: &'static str, pool: Address, key: u32) {
    KEYS.insert((venue, pool), key);
}

/// Ключ пула из памяти. Принимает: площадку DEX, пул. None — пул с настройкой ещё не проверялся. Ошибок нет.
pub fn get(venue: &'static str, pool: Address) -> Option<u32> {
    KEYS.get(&(venue, pool))
}

/// Комиссия для QuoterV2 (параметр fee): без настройки — fee_pips пула (как раньше), с настройкой — ключ реестра.
/// Принимает: задана ли настройка, ключ из памяти, fee_pips. Ошибка — причина (нет ключа / нет fee).
pub fn quoter_fee(set: bool, key: Option<u32>, fee_pips: Option<u32>) -> Result<u32, String> {
    match (set, key, fee_pips) {
        (false, _, Some(f)) => Ok(f),
        (false, _, None) => Err("у пула V3 нет fee".into()),
        (true, Some(k), _) => Ok(k),
        (true, None, _) => Err(format!(
            "ключ реестра фабрики ({SETTING}) этого пула не прочитан — квотер не спрашиваем (решение 281)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Селектор defaultFee() = 0x5a6c72d0 (keccak256 сигнатуры).
    #[test]
    fn default_fee_selector() {
        assert_eq!(
            selector("defaultFee()").to_vec(),
            vec![0x5a, 0x6c, 0x72, 0xd0]
        );
    }

    #[test]
    fn quoter_fee_choice() {
        assert_eq!(quoter_fee(false, Some(100), Some(200)), Ok(200));
        assert_eq!(quoter_fee(true, Some(100), Some(200)), Ok(100));
        assert!(quoter_fee(true, None, Some(200)).is_err());
        assert!(quoter_fee(false, None, None).is_err());
    }
}
