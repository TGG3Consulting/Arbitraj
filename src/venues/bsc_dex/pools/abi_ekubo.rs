//! Файл: venues/bsc_dex/pools/abi_ekubo.rs
//! Что делает: ABI и раскладка хранилища Ekubo (решение 447, вид ekubo): ключ пула и PoolId, слоты ядра Core (состояние
//! пула, тик, слово битмапа) и разбор их значений, вызов Core.sload (несколько слотов одним вызовом), реестр ключей
//! PoolKeyIndex.poolKeyById, квотер Router.quote (MEVCaptureRouter), события ядра (PoolInitialized, PositionUpdated) и
//! анонимный лог обмена (log0: locker 20 байт | PoolId | balanceUpdate | stateAfter); решение 454 — события TWAMM
//! (OrderUpdated, анонимный VirtualOrdersExecuted) и PoolId пула ордера.
//! Кто вызывает: pools/verify_ekubo.rs, load_ekubo.rs, events_ekubo.rs, ekubo_route.rs, quote/quoter_ekubo.rs, exec (шаг EKUBO).
//! От чего зависит: alloy::sol!, alloy-primitives.
//! Настройки: нет (адреса — dex.ekubo_core, dex.ekubo_key_index, dex.quoter [DEX]).
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa (ядро Arbitrum): src/types/poolKey.sol L11-L18, L34-L39 (ключ,
//! PoolId = keccak256 96 байт ключа); src/libraries/CoreStorageLayout.sol L21-L31 (смещения), L47-L49 (состояние — слот
//! PoolId), L64-L68 (тик — PoolId + TICKS_OFFSET + tick), L89-L93 (битмап — PoolId + BITMAPS_OFFSET + слово);
//! src/base/ExposedStorage.sol L151-L156 (sload(): слоты подряд после селектора, ответ — значения подряд);
//! src/types/poolState.sol L10-L46; src/types/tickInfo.sol L12-L35; src/types/poolBalanceUpdate.sol L7-L17;
//! src/types/positionId.sol L17-L29; src/interfaces/ICore.sol L111-L141 (события); src/Core.sol L840-L847 (log0
//! обмена, 116 байт); src/PoolKeyIndex.sol L16 (poolKeyById — геттер структуры: три поля). Router.quote —
//! evm-contracts@v3.1.1 src/Router.sol L523-L552 (MEVCaptureRouter v3.1.1 — docs.ekubo.org evm-v3, legacy).
//! TWAMM — @61d09daa src/interfaces/extensions/ITWAMM.sol L21 (OrderUpdated), src/extensions/TWAMM.sol L70-L81.
//! Селектор sload() = 0x380eb4e0 (keccak256("sload()")).

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};

alloy::sol! {
    struct EkuboPoolKey {
        address token0;
        address token1;
        bytes32 config;
    }

    interface IEkuboKeyIndex {
        function poolKeyById(bytes32 poolId) external view returns (address token0, address token1, bytes32 config);
    }

    interface IEkuboRouter {
        function quote(EkuboPoolKey poolKey, bool isToken1, int128 amount, uint96 sqrtRatioLimit, uint256 skipAhead) external returns (bytes32 balanceUpdate, bytes32 stateAfter);
    }

    interface IEkuboCore {
        event PoolInitialized(bytes32 poolId, EkuboPoolKey poolKey, int32 tick, uint96 sqrtRatio);
        event PositionUpdated(address locker, bytes32 poolId, bytes32 positionId, int128 liquidityDelta, bytes32 balanceUpdate, bytes32 stateAfter);
        event PositionFeesCollected(address locker, bytes32 poolId, bytes32 positionId, uint128 amount0, uint128 amount1);
        event FeesAccumulated(bytes32 poolId, uint128 amount0, uint128 amount1);
    }

    interface IEkuboTwamm {
        event OrderUpdated(address owner, bytes32 salt, EkuboPoolKey orderKey, int112 saleRateDelta);
    }
}

/// Селектор Core.sload() (ExposedStorage.sol L151).
pub const SLOAD: [u8; 4] = [0x38, 0x0e, 0xb4, 0xe0];
/// Длина данных анонимного лога обмена (Core.sol L841-L846).
pub const SWAP_LOG_LEN: usize = 116;
/// Решение 454: длина анонимного лога VirtualOrdersExecuted TWAMM (PoolId | скорость 0 | скорость 1 — 32 + 14 + 14;
/// TWAMM.sol L70-L81).
pub const TWAMM_LOG_LEN: usize = 60;

/// Решение 454: PoolId пула ордера TWAMM по ключу ордера (OrderKey: token0, token1, config — комиссия в старших 64 битах
/// config; orderConfig.sol L13-L17) и адресу TWAMM: config пула = TWAMM << 96 | комиссия << 32 (полный диапазон;
/// orderKey.sol L55-L61). Ошибок нет.
pub fn order_pool_id(twamm: Address, token0: Address, token1: Address, order_config: B256) -> B256 {
    let mut c = [0u8; 32];
    c[..20].copy_from_slice(twamm.as_slice());
    c[20..28].copy_from_slice(&order_config[..8]);
    pool_id(token0, token1, B256::from(c))
}

/// Смещения слотов ядра (CoreStorageLayout.sol L21, L23, L31) — hex без 0x.
const TICKS_OFFSET: &str = "435a5eb89a296820174331cf5a3902d9fca683928d56726d8e7acd6efb28c568";
const BITMAPS_OFFSET: &str = "3def450d0010a2fef515ce5eba4b363b5a0f42fdd4c53e1c737975db05a2e3a5";

/// Смещение из hex-константы исходника. Ошибок нет (константы — 64 hex-знака).
fn offset(hex: &str) -> U256 {
    U256::from_str_radix(hex, 16).unwrap_or_default()
}

/// PoolId = keccak256(token0 | token1 | config) по 32 байта (poolKey.sol L34-L39). Ошибок нет.
pub fn pool_id(token0: Address, token1: Address, config: B256) -> B256 {
    let mut buf = [0u8; 96];
    buf[12..32].copy_from_slice(token0.as_slice());
    buf[44..64].copy_from_slice(token1.as_slice());
    buf[64..96].copy_from_slice(config.as_slice());
    keccak256(buf)
}

/// Слот тика: PoolId + TICKS_OFFSET + tick (по модулю 2^256, тик со знаком). Ошибок нет.
pub fn tick_slot(id: B256, tick: i32) -> B256 {
    let base = U256::from_be_bytes(id.0).wrapping_add(offset(TICKS_OFFSET));
    let t = U256::from(tick.unsigned_abs());
    let v = if tick < 0 {
        base.wrapping_sub(t)
    } else {
        base.wrapping_add(t)
    };
    B256::from(v.to_be_bytes::<32>())
}

/// Слот слова битмапа: PoolId + BITMAPS_OFFSET + слово. Ошибок нет.
pub fn bitmap_slot(id: B256, word: u64) -> B256 {
    let v = U256::from_be_bytes(id.0)
        .wrapping_add(offset(BITMAPS_OFFSET))
        .wrapping_add(U256::from(word));
    B256::from(v.to_be_bytes::<32>())
}

/// Данные вызова Core.sload(): селектор и слоты подряд. Ошибок нет.
pub fn sload_data(slots: &[B256]) -> Bytes {
    let mut out = SLOAD.to_vec();
    for s in slots {
        out.extend_from_slice(s.as_slice());
    }
    Bytes::from(out)
}

/// Ответ sload(): значения подряд по 32 байта. None — длина не кратна 32 или не равна числу слотов. Ошибок нет.
pub fn sload_words(raw: &[u8], n: usize) -> Option<Vec<B256>> {
    (raw.len() == n * 32).then(|| raw.chunks(32).map(B256::from_slice).collect())
}

/// Состояние пула (poolState.sol L34-L40): (сжатая цена, тик, ликвидность). Нулевое слово — пул не создан. Ошибок нет.
pub fn decode_state(w: &B256) -> (U256, i32, u128) {
    let v = U256::from_be_bytes(w.0);
    let sqrt = v >> 160usize;
    let tick = i32::from_be_bytes([w[12], w[13], w[14], w[15]]);
    let liquidity = u128::from_be_bytes(low16(w));
    (sqrt, tick, liquidity)
}

/// TickInfo (tickInfo.sol L24-L29): (liquidityNet — старшие 128 бит, liquidityDelta — младшие со знаком). Ошибок нет.
pub fn decode_tick(w: &B256) -> (u128, i128) {
    let mut hi = [0u8; 16];
    hi.copy_from_slice(&w[..16]);
    (u128::from_be_bytes(hi), i128::from_be_bytes(low16(w)))
}

/// PoolBalanceUpdate (poolBalanceUpdate.sol L7-L17): (delta0 — старшие 128 бит, delta1 — младшие), со знаком. Ошибок нет.
pub fn decode_balance(w: &B256) -> (i128, i128) {
    let mut hi = [0u8; 16];
    hi.copy_from_slice(&w[..16]);
    (i128::from_be_bytes(hi), i128::from_be_bytes(low16(w)))
}

/// Границы позиции (positionId.sol L17-L29): (tickLower — биты 32..63, tickUpper — биты 0..31). Ошибок нет.
pub fn position_bounds(w: &B256) -> (i32, i32) {
    let lower = i32::from_be_bytes([w[24], w[25], w[26], w[27]]);
    let upper = i32::from_be_bytes([w[28], w[29], w[30], w[31]]);
    (lower, upper)
}

/// Младшие 16 байт слова. Ошибок нет.
fn low16(w: &B256) -> [u8; 16] {
    let mut b = [0u8; 16];
    b.copy_from_slice(&w[16..]);
    b
}

/// Анонимный лог обмена ядра: (PoolId, balanceUpdate, stateAfter). None — длина не 116 байт. Ошибок нет.
pub fn decode_swap_log(data: &[u8]) -> Option<(B256, B256, B256)> {
    if data.len() != SWAP_LOG_LEN {
        return None;
    }
    let id = B256::from_slice(&data[20..52]);
    let balance = B256::from_slice(&data[52..84]);
    let state = B256::from_slice(&data[84..116]);
    Some((id, balance, state))
}
