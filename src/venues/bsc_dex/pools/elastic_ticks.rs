//! Файл: venues/bsc_dex/pools/elastic_ticks.rs
//! Что делает: решение 475 (T451-03) — все инициализированные тики пула KyberSwap Elastic частями: вызовы
//! TicksFeesReader.getTicksInRange(pool, start, length) с длиной части из dex.elastic_ticks_chunk, первая часть — от
//! MIN_TICK, каждая следующая — от последнего тика прошлой, пока не встретится MAX_TICK.
//! Почему частями: при length = 0 читатель выделяет массив на ВСЕ возможные тики пула — (MAX_TICK − 1) / tickDistance · 2
//! + 3 слова у v2 (ks-elastic-sc 4ab08c0a contracts/periphery/TicksFeesReader.sol L46-L61), (MAX_TICK − MIN_TICK) /
//! tickDistance у v1 (Sourcify 42161 0x165c…E75D); стоимость памяти 3a + a²/512 газа (Yellow Paper) — у tickDistance 8 и
//! 10 это ≈ 96 и 62 млн газа, узел ответил «out of gas» (запись 20.11.2026, решение 462). С length > 0 массив — length
//! слов (L57-L58), цикл L61-L66 одинаков у v1 и v2: allTicks[i] = startTick; на MAX_TICK — break; иначе (, startTick) =
//! pool.initializedTicks(startTick). Значит часть = [start, следующий, …] из length тиков, после MAX_TICK — нули.
//! Кто вызывает: pools/load_elastic.rs (load), pools/prefetch_plan.rs (first_call — та же первая часть заранее).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_elastic.rs, pools/load_cl.rs (i24), math/tick_math.rs.
//! Настройки: dex.elastic_ticks_chunk, dex.tick_reader [DEX].

use alloy_primitives::Address;
use alloy_primitives::aliases::I24;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
use super::super::node::call;
use super::abi_elastic::ITicksFeesReader;
use super::families::Family;
use super::load_cl::i24;

/// Итог одной части: дальше читать с тика или список дочитан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Следующая часть начинается с этого тика (последний тик прочитанной части).
    From(i32),
    /// Встретился MAX_TICK (или пул без списка — пустая первая часть): тики дочитаны.
    Done,
}

/// Длина части из dex.elastic_ticks_chunk. Принимает: окружение, вид. Возвращает: длину (≥ 2: меньше — нет продвижения,
/// первый тик части повторяет последний прошлой). Ошибки: NotConfigured (не задана или меньше 2), Internal.
pub fn chunk(ctx: &Ctx, f: &Family) -> GatewayResult<u32> {
    let n = ctx.dex_int("dex.elastic_ticks_chunk", f.id)?;
    match u32::try_from(n) {
        Ok(v) if v >= 2 => Ok(v),
        _ => Err(ctx.err(
            ErrorCode::NotConfigured,
            format!(
                "dex.elastic_ticks_chunk [dex:{}] = {n} — нужно целое от 2 до 4294967295 (uint32 length \
                 getTicksInRange; часть повторяет последний тик прошлой)",
                f.id
            ),
        )),
    }
}

/// Вызов части у читателя. Принимает: окружение, пул, первый тик части, длину. Ошибки: тик вне int24.
pub fn part_call(
    ctx: &Ctx,
    pool: Address,
    start: i32,
    length: u32,
) -> GatewayResult<ITicksFeesReader::getTicksInRangeCall> {
    Ok(ITicksFeesReader::getTicksInRangeCall {
        pool,
        startTick: i24(ctx, start)?,
        length,
    })
}

/// Все инициализированные тики списка (от MIN_TICK до MAX_TICK включительно) частями на одном блоке.
/// Принимает: окружение, вид, адрес читателя, пул, блок-тег. Возвращает: тики по возрастанию.
/// Ошибки: NotConfigured (dex.elastic_ticks_chunk), узел, ответ не по ABI, разрыв списка (BadResponse).
pub async fn read(
    ctx: &Ctx,
    f: &Family,
    reader: Address,
    pool: Address,
    tag: &str,
) -> GatewayResult<Vec<i32>> {
    let length = chunk(ctx, f)?;
    let mut out = Vec::new();
    let mut start = MIN_TICK;
    loop {
        let part = call::view(ctx, reader, &part_call(ctx, pool, start, length)?, tag).await?;
        match merge(&mut out, start, &part, length) {
            Ok(Next::From(t)) => start = t,
            Ok(Next::Done) => return Ok(out),
            Err(why) => {
                return Err(ctx.err(
                    ErrorCode::BadResponse,
                    format!("getTicksInRange пула {pool:#x} от тика {start}: {why}"),
                ));
            }
        }
    }
}

/// Добавляет часть к прочитанным тикам. Принимает: прочитанное (дополняется), первый тик части, ответ читателя, длину.
/// Возвращает: откуда читать дальше или Done. Ошибки (текст): длина ответа не та, первый тик не равен запрошенному,
/// тики не по возрастанию, пустая часть не от MIN_TICK (тик из списка обязан быть в списке).
pub fn merge(out: &mut Vec<i32>, start: i32, part: &[I24], length: u32) -> Result<Next, String> {
    if part.is_empty() {
        // L43-L44: начало не в списке → пустой массив. От MIN_TICK — пул без списка (не инициализирован): тиков нет.
        return if start == MIN_TICK && out.is_empty() {
            Ok(Next::Done)
        } else {
            Err("пустая часть — тик не в списке initializedTicks".into())
        };
    }
    if part.len() != length as usize {
        return Err(format!("в ответе {} тиков, запрошено {length}", part.len()));
    }
    let got = super::load_elastic::parse_ticks(part);
    if got[0] != start {
        return Err(format!("первый тик части {} ≠ запрошенному", got[0]));
    }
    if got.windows(2).any(|w| w[0] >= w[1]) {
        return Err("тики части не по возрастанию".into());
    }
    // Первый тик части = последний тик прошлой (начало следующей части) — не повторяем.
    let skip = usize::from(!out.is_empty());
    out.extend_from_slice(&got[skip..]);
    let last = got[got.len() - 1];
    if last >= MAX_TICK {
        return Ok(Next::Done);
    }
    // Часть полна и MAX_TICK не встретился: последний тик > start (length ≥ 2, тики по возрастанию) — продвижение есть.
    if last <= start {
        return Err("нет продвижения по списку (длина части меньше 2)".into());
    }
    Ok(Next::From(last))
}
