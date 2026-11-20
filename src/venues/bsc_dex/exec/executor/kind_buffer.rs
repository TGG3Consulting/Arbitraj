//! Файл: venues/bsc_dex/exec/executor/kind_buffer.rs
//! Что делает: решение 474 — шаг BALANCER_V3_BUF (27) для пары пула Balancer V3, у которой сторона идёт через буфер
//! ERC4626 хранилища (VaultV3.via, pools/load_balancer_v3_state.rs): обёртка входа (WRAP до обмена) и обёртки выхода
//! (UNWRAP после обмена) по направлению шага. Шаг есть только в исходнике контракта (сборка v5 не собрана): строится,
//! только если в коде исполнителя кошелька есть вызов erc4626BufferWrapOrUnwrap (BUFFER_MARK —
//! проверка в entry.rs fits / fits_ok).
//! Кто вызывает: exec/executor/entry.rs (params, fits, fits_ok).
//! От чего зависит: core/types (VaultPoolData), exec/executor/kind.rs (Params), ctx_wallet.rs (exec_info),
//! pools/exec_callback.rs (has_call), quote/quoter_balancer_v3_buffer.rs (wrappers).
//! Настройки: нет.

use alloy_primitives::Address;

use crate::core::types::PoolState;
use crate::core::types::pool_vault::VaultPoolData;

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_callback;
use super::super::super::quote::quoter_balancer_v3_buffer::wrappers;
use super::kind::Params;

/// Сигнатура вызова, который есть в коде исполнителя только вместе с шагом BALANCER_V3_BUF = 27 (IPools.sol
/// IBalancerV3Vault.erc4626BufferWrapOrUnwrap, селектор 0x43583be5; IVaultMain.sol L175-L177 — enum в ABI — uint8). В
/// сборках v1–v4 (contracts/out/, out/v3/, out/v2/, out/v1/) его нет — тест tests/bsc_dex_executor_abi.rs.
pub const BUFFER_MARK: &str = "erc4626BufferWrapOrUnwrap((uint8,uint8,address,uint256,uint256))";

/// Причина, по которой шаг BALANCER_V3_BUF у кошелька окружения не строится. Принимает: окружение, состояние пула.
/// Возвращает: None — стороны через буфер нет или в коде сверенной версии исполнителя кошелька есть BUFFER_MARK;
/// Some — причина (исполнителя нет — обмен через буфер только исполнителем; версия не сверена; вызова нет — нужна
/// сборка v5). Ошибок нет.
pub fn refusal(ctx: &Ctx, state: &PoolState) -> Option<String> {
    let PoolState::VaultPool(d) = state else {
        return None;
    };
    if !has_buffer(d) {
        return None;
    }
    match ctx.exec_info() {
        None => Some(
            "обмен через буфер ERC4626 (шаг BALANCER_V3_BUF) — только исполнителем, у кошелька его нет".into(),
        ),
        Some((_, None)) => {
            Some("исполнитель кошелька не сверен — шаг BALANCER_V3_BUF не проверить".into())
        }
        Some((_, Some(v))) if exec_callback::has_call(v, BUFFER_MARK) => None,
        Some((_, Some(v))) => Some(format!(
            "исполнитель {} не знает шаг BALANCER_V3_BUF = 27 (обмен через буфер ERC4626, решение 474) — нужна \
             сборка исполнителя v5",
            v.label()
        )),
    }
}

/// Есть ли у пары сторона через буфер. Принимает: состояние пула. Ошибок нет.
pub fn has_buffer(d: &VaultPoolData) -> bool {
    wrappers(d, true).is_some()
}

/// Шаг BALANCER_V3_BUF по направлению. Принимает: состояние пула, адрес пула, вход шага — token0 пары. Возвращает:
/// None — сторон через буфер нет (шаг BALANCER_V3); иначе шаг с обёртками входа и выхода (0 — сторона без буфера).
/// Ошибок нет.
pub fn params(d: &VaultPoolData, pool: Address, zero_in: bool) -> Option<Params> {
    let (w_in, w_out) = wrappers(d, zero_in)?;
    Some(Params::BalancerV3Buf { pool, w_in, w_out })
}
