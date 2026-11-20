//! Файл: venues/bsc_dex/pools/settle_fusion.rs
//! Что делает: решение 463 (повторное ревью t236, блокер) — пул THENA Fusion (Proto::Algebra; другой вид с этим
//! протоколом — нет, families.rs) «в обновлении» (settle.rs Need::NextFee) от лога Swap или Fee до записи комиссии
//! СЛЕДУЮЩЕГО блока, прочитанной на блоке события (integral_fee.rs apply → fusion_fee.rs read_fee → set_fee). Раньше:
//! Swap применял новую цену, и всё время чтения пул был достоверен с прежней комиссией; Fee (AlgebraPool.sol
//! _getNewFee — первый своп блока, лог до Swap того же вызова) уходил ядру как Updated с комиссией ТЕКУЩЕГО блока
//! (globalState.fee). По образцу ревью 325 Н1 (SwapFee → Swap у Algebra Integral, settle.rs opens): отметка ставится
//! ДО чтения, ядру пул не уходит; снимает её только запись комиссии следующего блока (fee_written: достоверность — как
//! до отметки, was_valid). Fee открывает отметку и ждёт Swap той же транзакции (next_fee_open: его «только комиссия»
//! идёт при недостоверном из-за этой отметки пуле); лог другого действия — отметка потеряна, полное чтение
//! (settle_lost.rs lost, как Follow); голова выше блока отметки — полное чтение (settle_close.rs). Сбой чтения — пул
//! недостоверен, отметка долой (listen_log.rs, как у любой ошибки чтения).
//! Кто вызывает: settle.rs (step → next_fee), listen_log.rs (next_fee_open), integral_fee.rs (fee_written).
//! От чего зависит: settle.rs (Holds, Need), registry.rs, events.rs (Applied), families.rs, abi_algebra.rs (Fee).
//! Настройки: нет (базовое поведение).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;

use crate::core::types::PoolRef;

use super::abi_algebra::IAlgebraPool;
use super::events::Applied;
use super::families::{Family, Proto};
use super::registry::Registry;
use super::settle::{Holds, Need};

impl Holds {
    /// Лог THENA Fusion, после которого нужна комиссия следующего блока: Swap (Applied::Fee) или Fee (Updated) — пул
    /// «в обновлении» (Need::NextFee, транзакция лога). Принимает: реестр, (ссылка, вид), лог, итог применения.
    /// Возвращает: true — отметка стоит (ядру не отправлять до fee_written). Ошибок нет.
    pub fn next_fee(
        &self,
        reg: &Registry,
        at: (&PoolRef, &Family),
        log: &Log,
        a: &Applied,
    ) -> bool {
        let (r, f) = at;
        if f.proto != Proto::Algebra {
            return false;
        }
        let fee_log = log
            .topic0()
            .is_some_and(|t| *t == IAlgebraPool::Fee::SIGNATURE_HASH);
        if !matches!(a, Applied::Fee(_)) && !(fee_log && matches!(a, Applied::Updated)) {
            return false;
        }
        let block = log.block_number.unwrap_or_default();
        self.hold(reg, r, (block, Need::NextFee), Vec::new(), false);
        let tx = log.transaction_index;
        self.with(|m| {
            if let Some(h) = m.get_mut(r) {
                h.tx = tx;
            }
        });
        true
    }

    /// Комиссия следующего блока записана (integral_fee.rs set_fee): отметка Need::NextFee снимается, достоверность —
    /// как до отметки (settle.rs release). Другая отметка не трогается. Ошибок нет.
    pub fn fee_written(&self, reg: &Registry, r: &PoolRef) {
        if self.get(r).is_some_and(|h| h.need == Some(Need::NextFee)) {
            self.release(reg, r);
        }
    }

    /// Стоит ли Need::NextFee от лога той же транзакции того же блока (Fee перед Swap одного вызова). Принимает:
    /// ссылку, блок, транзакцию. Ошибок нет.
    pub fn next_fee_open(&self, r: &PoolRef, block: u64, tx: Option<u64>) -> bool {
        self.get(r).is_some_and(|h| {
            h.need == Some(Need::NextFee) && h.block == block && h.tx == tx
        })
    }
}
