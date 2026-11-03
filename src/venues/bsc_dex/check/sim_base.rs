//! Файл: venues/bsc_dex/check/sim_base.rs
//! Что делает: базовый блок eth_simulateV1 (check/simulate.rs) — номер и время симулированного блока
//! (blockOverrides.time). Решение 127: у ВСЕХ симуляций свопа перед отправкой (прямой и умный путь Universal Router,
//! бандл, исполнитель, цикл DEX ↔ DEX, сухие сделки) и у проб налогов время симулированного блока = время головы + 1 с,
//! а не +12 с узла BSC; один источник — head() (та же голова, на которой идёт симуляция и от которой считан срок).
//! Кто вызывает: check/{simulate, probe, native_probe, service_tax}.rs, exec/{trade_direct, trade_smart, bundle_send,
//! bundle_trade, cycle_bundle_trade, cycle_dry, cycle_exec, dry_build}.rs, exec/executor/{sim, trade}.rs.
//! От чего зависит: node/heads.rs (номер и время головы).
//! Настройки: нет (узел — bsc.node_http_url; свежесть головы — feed.stale_after_ms, feed.max_block_age_ms).

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;

/// Шаг времени симулированного блока к голове, секунды. Не настройка, а правило протокола: время блока в BSC — целые
/// секунды Unix (заголовок, поле timestamp), а eth_simulateV1 у BSC требует время СТРОГО больше базового
/// (bsc 947ba1cb internal/ethapi/simulate.go#L584-L588); 1 — наименьшее допустимое, ближе всего к блоку, в который
/// реально попадёт транзакция (не раньше следующего блока). Больше — проба строже реального включения, меньше — узел
/// отвергает запрос.
const NEXT_BLOCK_S: u64 = 1;

/// Базовый блок симуляции: номер (а не «latest»: срок транзакции считан по времени этого же блока, решение 61) и
/// время симулированного блока. Время не задано (Base::from(номер)) — узел ставит сам: у BSC время базового +
/// timestampIncrement = 12 с (bsc 947ba1cb internal/ethapi/simulate.go#L56-L57, #L577-L583) — так симулируются только
/// вызовы без срока и без зависимости от времени (проверка поддержки метода). Задано (Base::timed, решение 127) —
/// blockOverrides.time (execution-apis 16caee27 src/schemas/execute.yaml#L100-L117: «Time must either increase or
/// remain constant relative to the previous block»; BSC требует строго больше времени базового — simulate.go#L584-L588).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub number: u64,
    pub time: Option<u64>,
}

impl Base {
    /// Блок `number` со временем головы `head_time` (секунды): симулированный блок — на NEXT_BLOCK_S позже. Ошибок нет.
    pub fn timed(number: u64, head_time: u64) -> Self {
        Base {
            number,
            time: Some(head_time.saturating_add(NEXT_BLOCK_S)),
        }
    }
}

impl From<u64> for Base {
    fn from(number: u64) -> Self {
        Base { number, time: None }
    }
}

/// Общий источник для симуляции перед отправкой (решение 127): свежая голова (node/heads.rs Heads::at — номер и время
/// одного и того же блока). Возвращает: базовый блок с временем головы + 1 с и время головы (секунды Unix) — от него
/// вызывающий считает срок транзакции (now + dex.deadline_s), так срок и симуляция — от одного блока.
/// Ошибки: как у Heads::at (StaleData — голова устарела; узел; BadResponse — нет timestamp).
pub async fn head(ctx: &Ctx, heads: &Heads) -> GatewayResult<(Base, u64)> {
    let (number, time) = heads.at(ctx).await?;
    Ok((Base::timed(number, time), time))
}
