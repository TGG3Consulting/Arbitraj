//! Файл: venues/bsc_dex/pools/listen_log.rs
//! Что делает: один лог подписки пулов — найти пул (по адресу или по PoolId у менеджера), отбросить уже учтённое
//! (позиция (блок, logIndex) не позже последней учтённой), применить событие, перечитать бины или весь пул (реорг,
//! расхождение), отправить
//! пул ядру. Ошибка чтения — пул недостоверен, событие Invalidated, сессия переподключается; кроме StaleData (у узла
//! уже нет состояния блока события, решение 298, stale_read.rs) — тогда сессия идёт дальше, пул ждёт повтора.
//! Там же: недостоверность пачки пулов (invalidate) и USDT найденного пула (found_usdt) — для listen.rs; пересчёт
//! глубины пула на каждой отправке ядру (pools/depth.rs touch, решение 73).
//! Третье ревью t236: лог, после которого пул «в обновлении» (Integral SwapFee, LB CompositionFees, THENA Fusion Swap /
//! Fee — settle_fusion.rs shuts), делает пул недостоверным в том же изменении реестра, что его применяет; недостоверная
//! в конце запись ядру не отправляется (как у доставки из набора — emit_pool.rs Kept, Live: только valid).
//! Кто вызывает: pools/session.rs (лог глубокого пула), pools/listen.rs (invalidate).
//! От чего зависит: ctx.rs, node/rpc.rs, events.rs, load.rs, settle.rs, settle_bins.rs (бины события; ревью 290,
//! решение 357), integral_fee.rs (решение 325), registry.rs, stale_read.rs (решение 298), pool_fail.rs (решение 339),
//! refresh_event.rs (перечитывание по событию; решение 313 — замер), emit_pool.rs (решение 321), fusion_book.rs
//! (orphan — решение 463), settle_fusion.rs (решение 463, ревью t236: shuts — недостоверность в том же изменении
//! реестра, что применяет лог; preset — отметка с достоверностью до лога; next_fee_open — Fee перед Swap той же tx).
//! Решение 313: события пула, не ушедшие ядру из-за полного канала, считаются (emit → lost_events.rs). Решение 321:
//! emit вынесен в emit_pool.rs (здесь — реэкспорт для прежних вызывающих) — при dex.pool_events_keep = да событие не
//! теряется, а ждёт места в канале (gateway/core_out.rs, решение 324).
//! Решение 314: перечитывания по событию (off_map, Refresh, Bins) — классом dex.event_refresh_urgency (refresh_urgency.rs).
//! Решение 325: Applied::Fee (Algebra Integral) — только комиссия (integral_fee.rs) при dex.event_refresh_fee_only.
//! Не задана — как раньше. Бины событий (Infinity Bin, LB) — всегда одним запросом с активным бином (settle_bins.rs,
//! решение 357; прежний reread_bins.rs удалён — ревью слияния, Н5).
//! Решение 332: после бинов события (Infinity Bin, LB V2.1) — проверка «цена ушла за прочитанную полосу бинов» и
//! перечитывание пула целиком (off_map_bins.rs after, при dex.bin_reread_left_pct у DEX; не задана — как раньше).
//! Решение 357 (деньги, не под настройкой): пул не уходит ядру и не котируется в промежуточном состоянии одного действия
//! в сети — бины события и активный бин конца блока одним запросом (settle_bins.rs), логи одного обмена LB применяются
//! вместе; лог, за которым в том же вызове всегда идёт ещё лог пула (SwapFee Integral, CompositionFees LB), — пул «в
//! обновлении» до него (settle.rs step); полное перечитывание и ошибка чтения снимают отметку (forget). SwapFee с
//! «только комиссией» (решение 325) — сначала комиссия на блоке события, затем отметка (ядру — после Swap).
//! Решение 339 (pool_fail.rs): при dex.pool_fail_pause_ms прочая ошибка чтения тоже не рвёт сессию — пул недостоверен,
//! в очередь повтора с растущей паузой; пул на паузе по событию не читается (как ждущий повтора). Ревью t153 (Б1):
//! событие недостоверного пула не на паузе, которое его не перечитало, — итог true (пул в очередь повтора); пул «в
//! обновлении» (решение 357) этим не возрождается — его ведёт settle.rs.
//! Настройки: через вызываемые (log.pool_refresh_timing, log.pool_events_lost_interval_s, dex.event_refresh_urgency,
//! dex.pool_events_keep, dex.event_refresh_fee_only, dex.pool_fail_pause_ms, dex.bin_reread_left_pct, dex.bin_depth_usd).
//!
//! removed: true при реорге — execution-apis subscribe.yaml#L1-L60; порядок применения — (blockNumber, logIndex),
//! как присылает узел (research/flow/bsc_dex.md, раздел 4).

use alloy::rpc::types::Log;
use alloy_primitives::U64;
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::node::rpc;
use super::super::quote::emptiness;
use super::discover::Found;
use super::events::{self, Applied};
use super::group::Msg;
use super::integral_fee;
use super::listen::{End, Pools};
use super::refresh_event::{self, Event};
use super::refresh_urgency;
use super::registry::Entry;
use super::settle_bins::Settled;

/// Один лог: найти пул, применить или перечитать, отправить ядру. `waiting` — пул в очереди повтора пачки (решение
/// 298): событие применяется, но перечитывания нет (узел на старом блоке не спрашивается, журнал не пишется) — позиция
/// события, требующего перечитывания, отмечается (stale_read::defer), повтор прочитает пул не раньше неё.
/// Возвращает: true — чтение на блоке события опоздало (StaleData, решение 298): пул недостоверен, Invalidated ушло,
/// вызывающий ставит пул в очередь повтора (stale_read::requeue); false — обработан как раньше.
/// `queued` — канал сессии пачки: сколько сообщений ждёт, пока пул перечитывается (замер решения 313).
/// Решение 339: при dex.pool_fail_pause_ms true и при прочей ошибке чтения (пул на паузе, pool_fail.rs); ревью t153
/// (Б1) — и когда пул недостоверен, не на паузе, не ждёт повтора, а событие его не перечитало.
/// Ошибки: End — прочая ошибка чтения без dex.pool_fail_pause_ms (пул недостоверен), лог без позиции, ядро закрыло канал.
pub async fn on_log(
    p: &Pools,
    log: &Log,
    waiting: bool,
    queued: &UnboundedReceiver<Msg>,
) -> Result<bool, End> {
    let r = match log.topics().get(1) {
        Some(id)
            if p.reg.contains(&PoolRef::Singleton {
                manager: log.address(),
                id: *id,
            }) =>
        {
            PoolRef::Singleton {
                manager: log.address(),
                id: *id,
            }
        }
        _ => PoolRef::Contract(log.address()),
    };
    let Some(entry) = p.reg.get(&r) else {
        return Ok(false);
    };
    // Решение 339: пул на паузе после неудач чтения — события без перечитывания, как у ждущего повтора.
    let waiting = waiting || super::pool_fail::paused(p, &r);
    // Ревью t153 (Б1): при dex.pool_fail_pause_ms событие НЕДОСТОВЕРНОГО пула не на паузе и не в очереди повтора
    // ставит его на перечитку (итог true — вызывающий ставит в очередь повтора), если событие само его не перечитало:
    // иначе пул, чьи события только применяются (Applied::Updated), остался бы недостоверным навсегда.
    // Решение 357: пул «в обновлении» недостоверен отметкой settle.rs — его снимает приход логов или голова, не повтор.
    // Четвёртое ревью t236: и пул «в обновлении», недостоверный уже ДО отметки (was_valid = false), — снятие отметки
    // вернёт ему недостоверность, а следующего лога у тихого пула может не быть: в очередь повтора после снятия (ниже —
    // `again`; лог, оставивший отметку, выходит раньше и в очередь не ставит).
    let revive = !waiting
        && !entry.valid
        && (!p.holds.pending(&r) || p.holds.get(&r).is_some_and(|h| !h.was_valid))
        && super::pool_fail::base(&p.ctx).is_some();
    let (Some(block), Some(index)) = (log.block_number, log.log_index) else {
        return Err(End::Broken(
            "лог подписки без blockNumber/logIndex — порядок событий неизвестен".into(),
        ));
    };
    // Решение 463 (ревью t236): реорг — данные оракула THENA Fusion с этого блока и позже долой (fusion_book.rs).
    if log.removed {
        super::fusion_book::orphan(&r, block);
    }
    if !log.removed && entry.has_applied(block, index) {
        return Ok(revive);
    }
    // Решение 357 (ревью слияния, Б2): пул «в обновлении» получил лог ДРУГОГО действия (Active — более поздний блок,
    // Follow — другая tx или блок): ожидаемые логи потеряны (обрыв, пропуск). Отметка — долой без возврата
    // достоверности, событие учтено позицией, пул — на полное чтение (итог true — очередь повтора), не «голова равна».
    if !waiting && p.holds.lost(&r, block, log.transaction_index) {
        p.holds.forget(&r);
        p.reg.update(&r, |e| e.valid = false);
        super::stale_read::defer(&p.reg, &r, block, index);
        return Ok(true);
    }
    // Третье ревью t236 (блокер 1): лог, после которого пул «в обновлении» (settle_fusion.rs shuts: Integral SwapFee,
    // LB CompositionFees, THENA Fusion Swap / Fee), — valid = false в ТОМ ЖЕ изменении реестра, что применяет лог:
    // котировка и граф (другие задачи) не видят новую цену с прежней комиссией ни на мгновение. `was` — достоверность
    // до лога (её вернёт снятие отметки, settle_fusion.rs preset).
    let (applied, was) = if log.removed {
        (Applied::Refresh("реорг: событие отменено (removed)".into()), None)
    } else {
        p.reg
            .update(&r, |e| {
                let was = e.valid;
                let a = events::apply(e, log);
                let shut = !waiting && super::settle_fusion::shuts(e.family, log, &a);
                if shut {
                    e.valid = false;
                }
                (a, shut.then_some(was))
            })
            .unwrap_or((Applied::Ignored, None))
    };
    // Решение 325: «только комиссия» — при dex.event_refresh_fee_only, пуле не в очереди повтора, достоверном и с
    // состоянием Algebra (ревью 325, Н2/Н3 — integral_fee::fits); иначе прежний Refresh.
    // Решение 463 (повторное ревью t236): THENA Fusion после Fee той же tx — «только комиссия» (settle_fusion.rs).
    let open = p.holds.next_fee_open(&r, block, log.transaction_index);
    let applied = match applied {
        Applied::Fee(why) if !integral_fee::fits(&p.ctx, &entry, waiting, open) => {
            Applied::Refresh(why)
        }
        x => x,
    };
    // Решение 357: за этим логом в том же вызове по коду контракта идёт ещё лог пула — пул «в обновлении», ядру не
    // уходит; следующий лог пула той же tx снимает отметку (settle.rs step). Ревью слияния (Н1): у Applied::Fee
    // (решение 325) отметка ставится ДО чтения комиссии (ветка Fee ниже читает её и выходит). Третье ревью t236: сама
    // недостоверность — уже в изменении реестра, применившем лог (выше); здесь — отметка с достоверностью до лога.
    if let Some(was) = was
        && super::settle_fusion::shuts(entry.family, log, &applied)
    {
        p.holds.preset(&r, block, was);
    }
    let held = p
        .holds
        .step(&p.reg, (&r, entry.family), log, &applied, waiting);
    if held && !matches!(applied, Applied::Fee(_)) {
        return Ok(false);
    }
    let res = match applied {
        Applied::Ignored => return Ok(revive),
        // Решение 298: пул ждёт повтора — не читаем (повтор на новых блоках прочитает его не раньше этого события).
        Applied::Refresh(_) if waiting => {
            if !log.removed {
                super::stale_read::defer(&p.reg, &r, block, index);
            }
            return Ok(false);
        }
        Applied::Updated | Applied::Bins(_) if waiting => Ok(()),
        // Цена ушла так, что полоса глубины вышла за прочитанные слова (решение 73) — карта вокруг нового тика сразу,
        // а не при переподключении: иначе глубина считалась бы не целиком и вердикт мигал бы.
        Applied::Updated => match p.reg.get(&r) {
            Some(e) if super::depth::off_map(&p.ctx, &e) => {
                refresh_event::off_map(p, &e, block, queued)
                    .await
                    .map(|x| p.reg.put(x))
            }
            _ => Ok(()),
        },
        // Решение 325: только комиссия на блоке события, дальше — как Updated (цена из события могла уйти за карту).
        Applied::Fee(_) => {
            // Ревью 325 (Н4): класс очереди и замер (причина «fee») — refresh_event.rs fee.
            match refresh_event::fee(p, &entry, &r, (block, log.log_index), queued).await {
                // Решение 357 при 325: SwapFee — пул уже «в обновлении» (step выше, до чтения) до Swap той же tx.
                // Решение 463: у THENA Fusion отметку снимает сама запись комиссии (settle_fusion.rs) — тогда дальше.
                Ok(()) if held && p.holds.pending(&r) => return Ok(false),
                Ok(()) => match p.reg.get(&r) {
                    Some(e) if super::depth::off_map(&p.ctx, &e) => {
                        refresh_event::off_map(p, &e, block, queued)
                            .await
                            .map(|x| p.reg.put(x))
                    }
                    _ => Ok(()),
                },
                Err(e) => Err(e),
            }
        }
        // Решение 314: бины на блоке события — классом dex.event_refresh_urgency (не задана — как раньше). Ревью 314:
        // настройка читается, только если есть что читать у узла (бины есть, ссылка — свой контракт или Singleton;
        // иначе бины к узлу не читаются: пустой список — call::try_many без запроса).
        Applied::Bins(ids)
            if ids.is_empty() || !matches!(r, PoolRef::Contract(_) | PoolRef::Singleton { .. }) =>
        {
            Ok(())
        }
        // Решение 332: после бинов события — прочитанного в сторону цены меньше dex.bin_reread_left_pct от
        // dex.bin_depth_usd (или активный бин за полосой) — пул целиком на блоке события (off_map_bins.rs).
        // Решение 357: бины события и активный бин конца блока — одним запросом; активный бин памяти ≠ конца блока —
        // логи блока пула ещё идут: пул «в обновлении», ядру не уходит (settle_bins.rs).
        Applied::Bins(ids) => {
            match refresh_urgency::urgent(&p.ctx, super::settle_bins::on_bins(p, &r, &ids, block))
                .await
            {
                Ok(Settled::Done) => super::off_map_bins::after(p, &r, block, queued).await,
                Ok(Settled::Held) => return Ok(false),
                Err(e) => Err(e),
            }
        }
        Applied::Refresh(why) => {
            p.ctx
                .log(
                    Level::Warn,
                    "dex.pool",
                    format!(
                        "{} {}: {why} — перечитываю",
                        entry.family.title, entry.ticker
                    ),
                    None,
                )
                .await;
            // Реорг: читаем текущий блок; иначе — конец блока события (поздние события блока уже учтены в нём).
            let at = if log.removed {
                refresh_urgency::urgent(&p.ctx, latest(p)).await
            } else {
                Ok(block)
            };
            match at {
                Ok(at) => {
                    refresh_event::refresh(p, &entry, at, &Event::new("refresh", block, queued))
                        .await
                        .map(|e| {
                            // Решение 357: пул прочитан целиком — «в обновлении» снято.
                            p.holds.forget(&r);
                            p.reg.put(e);
                            p.signals.pool_ready();
                        })
                }
                Err(e) => Err(e),
            }
        }
    };
    if let Err(e) = res {
        p.reg.update(&r, |x| x.valid = false);
        // Решение 357: недостоверен по ошибке — отметку «в обновлении» ведёт не она (повтор / переподключение).
        p.holds.forget(&r);
        super::invalidated::send(p, entry.coin, entry.ticker.clone());
        // Решение 298: состояния блока события у узла уже нет — сессию пачки не рвём, пул перечитается на новых блоках.
        if super::stale_read::is_stale(&e) {
            super::stale_read::note(p, &entry, block, &e).await;
            return Ok(true);
        }
        // Решение 339: задана dex.pool_fail_pause_ms — ошибка касается только этого пула: в очередь повтора с паузой.
        if super::pool_fail::note(p, &r, entry.family.title, &entry.ticker, &e).await {
            return Ok(true);
        }
        return Err(End::Broken(e.message));
    }
    match p.reg.get(&r) {
        Some(e) => {
            note_refill(p, &e);
            // Б1: событие пул не перечитало (запись всё ещё недостоверна) — в очередь повтора.
            let again = revive && !e.valid;
            // Третье ревью t236 (п. 5): недостоверная запись (пул ждёт повтора, «в обновлении», после ошибки чтения)
            // ядру не уходит — как у доставки из набора (emit_pool.rs Kept: Live — только valid). Ядро получит её,
            // когда достоверность вернётся: полное чтение (put и emit), снятие отметки логами того же действия
            // (settle.rs release / settle_fusion.rs fee_written — затем этот же путь), снятие Need::Clock
            // (fusion_head.rs set — emit при новой комиссии).
            if !e.valid {
                return Ok(again);
            }
            emit(p, e.pool).map(|()| again)
        }
        None => Ok(false),
    }
}

/// Пул, исключённый сверкой как пустой, снова не пуст — сигнал сверить его сразу (pools/progress.rs). Ошибок нет.
pub fn note_refill(p: &Pools, e: &Entry) {
    if emptiness::empty(e) {
        return;
    }
    if p.checks
        .pool(&e.pool.reference)
        .quoter
        .any_err(|why| why == emptiness::EMPTY)
    {
        p.signals.refilled(e.pool.reference.clone());
    }
}

/// Текущий номер блока узла (HTTP). Ещё — listen_site.rs (реорг у пар многомонетного пула, решение 295).
pub async fn latest(p: &Pools) -> GatewayResult<u64> {
    let b: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    Ok(b.to())
}

/// Пул — ядру без ожидания; решение 321 — при полном канале не теряется (dex.pool_events_keep): pools/emit_pool.rs.
pub use super::emit_pool::emit;

/// Пулы пачки недостоверны: отметка в реестре и событие Invalidated по каждой монете.
pub fn invalidate(p: &Pools, pools: &[(Found, String)]) {
    // Решение 211: одна монета — один адрес (раньше — набор тикеров).
    let mut coins = std::collections::BTreeMap::new();
    // Решение 357 (ревью слияния, Б2): отметки «в обновлении» пачки — долой (события окна обрыва потеряны; пулы вернёт
    // только полное чтение приёма пачки).
    p.holds.forget_all(pools.iter().map(|(f, _)| &f.reference));
    for (f, t) in pools {
        p.reg.update(&f.reference, |e| e.valid = false);
        coins.entry(f.coin).or_insert_with(|| t.clone());
    }
    for (coin, t) in coins {
        super::invalidated::send(p, coin, t);
    }
}

/// Адрес USDT найденного пула: вторая валюта после монеты.
pub fn found_usdt(f: &Found) -> alloy_primitives::Address {
    if f.token0 == f.coin {
        f.token1
    } else {
        f.token0
    }
}
