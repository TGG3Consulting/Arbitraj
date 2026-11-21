//! Файл: venues/bsc_dex/quote/usable_reuse.rs
//! Что делает: решение 336 (задача t157, п. 2а) — допуск пула для классики (quote/select.rs usable: достоверность,
//! глубина, DEX подключена, бан справочника, крючок, сверка с квотером, газ, налог пула) от объёма НЕ зависит, а
//! считался на каждой котировке каждой точки подбора объёма — около 15 чтений настроек и 4 общих замка на пул (разбор
//! docs/notes/t157-cpu-parallel-2026-11-16.md п. 1б). Внутри подбора объёма кандидата с DEX (короб с reuse — всегда,
//! кроме цикла DEX ↔ DEX; решение 363, в логику: была dex.quote_reuse) он считается один раз на пул и сторону и берётся
//! из короба подбора (gateway/calc_share.rs; короб переносится в поток счёта quote/cpu.rs → cpu_local.rs).
//! Свежесть: запись короба помнит поколение состояния пула (Entry.state_gen, pools/state_gen.rs), достоверность,
//! «спит» и поколение настроек (settings/store.rs generation — решение 363); не совпало с тем, по чему котируют сейчас,
//! или поколение 0 (запись вне реестра) — допуск заново.
//! Ключ короба — всё, от чего допуск зависит кроме пула: сторона, decimals USDT, плановая цена газа (пулы Solidly,
//! ревью 273), снимок (frozen_head), продажа выхода (решение 247 п. В).
//! Что в пределах одного подбора НЕ видно (видно со следующего кандидата и в финальной котировке — она идёт без
//! короба): смена фактов проверок пула без смены его состояния (квотер, налог, газ), бан справочника. Решение «335
//! (лесенка влияния)», ревью 3: смена итога крючка (лесенка, проба перед сделкой) видна сразу — запись помнит отметку
//! итога крючка (check/state.rs hook_mark). Так же,
//! как у графа (quote/graph_reuse.rs). Смена настроек видна сразу (поколение настроек).
//! Вне подбора (финальная котировка, исполнение, экран, проверки) короба нет — допуск каждый раз, как до 336.
//! Короба или reuse в нём нет (вне подбора, цикл DEX ↔ DEX) — допуск как до 336, на горячем пути только чтение поля
//! потока (ревью t157 Б1). Замер частей — только при reuse.
//! Замер (решение 312, log.quote_timing): этап «в т.ч. классика: допуск пулов», счётчики допусков и попаданий.
//! Кто вызывает: quote/select.rs (usable).
//! От чего зависит: quote/select.rs (usable_with), quote/cpu_local.rs, gateway/calc_share.rs, gateway/exit_scope.rs,
//! quote/quote_timing.rs.
//! Решение 409 (t194, ревью Н1): короб допусков — карта по частям (gateway/shard_map.rs), допуск за Arc (Admits).
//! Настройки: нет своих (поколение настроек — settings/store.rs generation).

use std::sync::Arc;

use alloy_primitives::U256;

use crate::core::types::{PoolRef, Side};
use crate::gateway::shard_map::ShardMap;

use super::super::pools::registry::Entry;
use super::Quoting;
use super::cpu_local;
use super::quote_timing::{self, Count, Stage};
use super::select::{self, Usable};

/// Допуск одного пула: на чём посчитан и итог.
struct Slot {
    /// Поколение состояния пула, достоверность, «спит» — как у записи реестра при допуске.
    state_gen: u64,
    valid: bool,
    asleep: bool,
    /// Решение 363: поколение настроек, прочитанное ДО допуска (смена во время допуска — тоже заново).
    settings_gen: u64,
    /// Решение «335 (лесенка влияния)», ревью 3: отметка итога крючка пула (check/state.rs hook_mark) — лесенка
    /// влияния в допуске; новая запись (цикл, проба перед сделкой) — допуск заново.
    hook: Mark,
    out: Result<Usable, String>,
}

/// Отметка итога крючка: блок замера и время итога.
type Mark = (Option<u64>, Option<chrono::DateTime<chrono::Utc>>);

/// Ключ допуска: пул и всё, от чего допуск зависит кроме пула (см. шапку) — без сборки строки (ревью t157 Б1).
#[derive(PartialEq, Eq, Hash)]
struct Key {
    pool: PoolRef,
    buy: bool,
    usdt_dec: u8,
    plan_gas: Option<U256>,
    frozen_head: Option<u64>,
    exit: bool,
}

/// Допуски одного подбора. Решение 409 (t194, п. в; ревью Н1): точки подбора считаются параллельно
/// (size.parallel_points) и читают короб на каждый пул — карта по частям (gateway/shard_map.rs: замок своей части, а не
/// один на подбор; новый допуск — вставка O(1), без копии карты), допуск за Arc (без копии записи).
#[derive(Default)]
struct Admits {
    map: ShardMap<Key, Arc<Slot>>,
}

/// Имя места в коробе подбора.
const SLOT: &str = "bsc_dex.usable_reuse";

/// Что делает: допуск пула (см. шапку). Принимает и возвращает: как select::usable. Ошибки: как select::usable.
pub fn usable(q: &Quoting, e: &Entry, side: Side, usdt_dec: u8) -> Result<Usable, String> {
    // Ревью t157 Б1: короба нет или в нём нет повторного использования — допуск как до 336; сверху только чтение поля
    // потока (короб) и признака замера (часы — только при log.quote_timing, решение 312).
    let share = cpu_local::share().filter(|s| s.reuse());
    if share.is_none() && !quote_timing::active() {
        return select::usable_with(q, e, side, usdt_dec, None);
    }
    // Решение 336: замер — время и число допусков (часы только при замере котировки).
    let t = quote_timing::now();
    let (out, hit) = match share {
        Some(sh) => admit(&sh.get_or_put(SLOT, Admits::default), q, e, side, usdt_dec),
        None => (select::usable_with(q, e, side, usdt_dec, None), false),
    };
    quote_timing::add(Stage::ClassicAdmit, t);
    quote_timing::count(Count::AdmitCalls, 1);
    quote_timing::count(Count::AdmitHits, u64::from(hit));
    out
}

/// Допуск пула из места короба или заново, и признак «взят из короба». Принимает: место, как usable.
/// Ошибок сверх usable нет.
fn admit(
    a: &Admits,
    q: &Quoting,
    e: &Entry,
    side: Side,
    usdt_dec: u8,
) -> (Result<Usable, String>, bool) {
    if e.state_gen == 0 {
        return (select::usable_with(q, e, side, usdt_dec, None), false);
    }
    let key = Key {
        pool: e.pool.reference.clone(),
        buy: side == Side::Buy,
        usdt_dec,
        plan_gas: q.plan_gas,
        frozen_head: q.frozen_head,
        exit: crate::gateway::exit_scope::is_exit(),
    };
    // Решение 363: поколение настроек — до допуска (запись помнит то, на чём допуск посчитан).
    let settings_gen = q.ctx.settings.generation();
    // Ревью 4 (Н6): отметка крючка — только у пулов с крючком B/C (у остальных замок итогов не берётся).
    let hook = if super::super::check::quoter_amount::hooked(e) {
        q.checks.hook_mark(&e.pool.reference)
    } else {
        (None, None)
    };
    let hit = a.map.with(&key, |s| {
        s.filter(|s| {
            s.state_gen == e.state_gen
                && s.valid == e.valid
                && s.asleep == e.asleep
                && s.settings_gen == settings_gen
                && s.hook == hook
        })
        .map(|s| s.out.clone())
    });
    if let Some(h) = hit {
        return (h, true);
    }
    let out = select::usable_with(q, e, side, usdt_dec, None);
    let slot = Slot {
        state_gen: e.state_gen,
        valid: e.valid,
        asleep: e.asleep,
        settings_gen,
        hook,
        out: out.clone(),
    };
    a.map.insert(key, Arc::new(slot));
    (out, false)
}
